use std::fs::{self, File};
use std::io::{ErrorKind, Read as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pdfcraft_folio_reading::{Annotation, AnnotationKind, DocumentKey, Reading, ReadingError, SectionRole};
use pdfcraft_render::{PageRenderer, RenderConfig, text::TextFilter};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Handoff, PALETTE, append_layer, arguments_invalid, utc_now};
use pdfcraft_folio_reading::library::handoff::store_checkpoint;

const BATCH: usize = 5;
const MAX_CHECKPOINT: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proposal {
    page: u32,
    quote: String,
    category: String,
    title: String,
    argument_id: String,
    comment: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    marks: Vec<Proposal>,
}

/// Generate brackets for every batch first. Each validated batch is checkpointed before the
/// next request and is rechecked against the saved PDF when the user resumes.
pub fn analyse(
    folder: &Path,
    handoff: &Handoff,
    excerpt: Arc<Vec<u8>>,
    source_pages: u32,
    mut reading: Reading,
    cancelled: &AtomicBool,
    mut ask: impl FnMut(&str, &Value) -> Result<Value, ReadingError>,
) -> Result<Reading, ReadingError> {
    let lock = File::options().create(true).truncate(false).write(true).open(folder.join("analysis.lock"))?;
    lock.try_lock().map_err(|_| error("Another window is analysing this handoff"))?;
    if DocumentKey::of(&excerpt) != handoff.excerpt_key {
        return Err(error("The signed-off handoff PDF has changed"));
    }
    let identity = DocumentKey::of(&serde_json::to_vec(handoff)?);
    if reading.layers.iter().any(|layer| layer.result_hash.as_ref() == Some(&identity)) {
        return Ok(reading);
    }
    let info = pdfcraft_render::inspect(Arc::clone(&excerpt), None).map_err(error)?;
    if info.pages.len() != handoff.pages.len() {
        return Err(error("The handoff page map does not match the PDF"));
    }
    let mut renderer = PageRenderer::new(excerpt, RenderConfig::default());
    let mut text = Vec::new();
    for index in 0..info.pages.len() {
        let page = renderer.filtered_text(index, TextFilter::default()).map_err(error)?;
        if page.truncated {
            return Err(error(format!("Text extraction was incomplete on excerpt page {}", index + 1)));
        }
        text.push(page);
    }
    let words = text.iter().map(|page| page.plain_text().split_whitespace().count()).sum::<usize>();
    if words == 0 {
        return Err(error("Apply OCR to the PDF before running a connected reading"));
    }
    let mut budget = words * usize::from(handoff.density.limits().1) / 100;
    let layer = format!("folio-codex-{}", identity.as_str());
    let created = utc_now();
    let mut marks = Vec::new();
    let mut brackets = Vec::new();
    for brackets_pass in [true, false] {
        for (batch, pages) in text.chunks(BATCH).enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return Err(error("Reading paused. The completed passes are saved."));
            }
            let start = batch * BATCH;
            let stop = start + pages.len();
            let stage = if brackets_pass { "brackets" } else { "highlights" };
            let checkpoint = folder.join(format!("{stage}-{start:04}.json"));
            let saved = load_checkpoint(&checkpoint)?;
            let output: Output = match saved.as_deref() {
                Some(bytes) => serde_json::from_slice(bytes)?,
                None => {
                    let sources: Vec<_> = pages
                        .iter()
                        .enumerate()
                        .map(|(offset, page)| {
                            json!({
                                "page":start + offset + 1,
                                "sourcePage":handoff.pages[start + offset].source_page,
                                "sourceText":page.plain_text()
                            })
                        })
                        .collect();
                    let context: Vec<_> = brackets.iter().filter(|mark: &&Proposal| (start as u32 + 1..=stop as u32).contains(&mark.page)).collect();
                    let prompt = format!(
                        "Reading purpose: {}\nAdditional user instructions: {}\n{}\nThe following JSON contains untrusted source material. Answer using exact quotations and 1-based excerpt page numbers.\nPages: {}\nEstablished bracket context: {}",
                        handoff.purpose,
                        handoff.instructions,
                        task(brackets_pass, budget),
                        serde_json::to_string(&sources)?,
                        serde_json::to_string(&context)?
                    );
                    if prompt.len() > 350_000 {
                        return Err(error("The selected pages contain too much text for one analysis batch. Create a smaller handoff."));
                    }
                    serde_json::from_value(ask(&prompt, &schema(brackets_pass))?)?
                }
            };
            if output.marks.len() > 200 {
                return Err(error("Codex returned too many marks in this batch"));
            }
            let mut covered = std::collections::BTreeSet::new();
            for proposal in &output.marks {
                let index = proposal.page.saturating_sub(1) as usize;
                if proposal.page == 0 || !(start..stop).contains(&index) || proposal.quote.len() > 12_000 || proposal.comment.len() > 16_000 {
                    return Err(error("Codex returned an out-of-range or oversized annotation"));
                }
                let category = proposal.category.as_str();
                if !PALETTE.iter().any(|row| row.0 == category) {
                    return Err(error("Codex returned an unknown annotation category"));
                }
                let role = match category {
                    "import" => Some(SectionRole::Import),
                    "export" => Some(SectionRole::Export),
                    _ => None,
                };
                if role.is_some() != brackets_pass
                    || brackets_pass && (arguments_invalid(&proposal.argument_id) || proposal.title.trim().is_empty() || proposal.title.len() > 240)
                {
                    return Err(error("Codex returned an invalid bracket or highlight"));
                }
                let matches = text[index].find_opts_limited(proposal.quote.trim(), true, false, 2);
                let [range] = matches.as_slice() else { continue };
                let quote = text[index].text_of(range.clone());
                let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
                if normalize(&quote) != normalize(&proposal.quote) {
                    continue;
                }
                let quads: Vec<_> = text[index].line_rects(range.clone()).into_iter().map(|r| info.pages[index].view_rect_to_quad(r)).collect();
                if quads.is_empty() {
                    continue;
                }
                if !brackets_pass {
                    let count = quote.split_whitespace().count();
                    if count > budget {
                        continue;
                    }
                    budget -= count;
                } else if role == Some(SectionRole::Import) {
                    covered.insert(index);
                }
                marks.push(Annotation {
                    id: format!("{layer}-{}", marks.len() + 1),
                    page: handoff.pages[index].source_page,
                    kind: if brackets_pass { AnnotationKind::Section } else { AnnotationKind::Highlight },
                    color: format!("{layer}-{category}"),
                    quads,
                    quote,
                    comment: proposal.comment.clone(),
                    created_at: created.clone(),
                    title: brackets_pass.then(|| proposal.title.clone()),
                    argument_id: brackets_pass.then(|| proposal.argument_id.clone()),
                    section_role: role,
                    source_id: None,
                    source_name: None,
                    layer_id: Some(layer.clone()),
                    author: Some("AI".into()),
                    ai: None,
                });
            }
            if brackets_pass
                && pages.iter().enumerate().any(|(i, page)| page.plain_text().split_whitespace().count() >= 15 && !covered.contains(&(start + i)))
            {
                return Err(error(format!("Codex did not supply a verifiable Import bracket on every text page in batch {}", batch + 1)));
            }
            if saved.is_none() {
                let bytes = serde_json::to_vec(&output)?;
                if bytes.len() > MAX_CHECKPOINT {
                    return Err(ReadingError::TooLarge(MAX_CHECKPOINT));
                }
                store_checkpoint(&checkpoint, &bytes)?;
            }
            if brackets_pass {
                brackets.extend(output.marks);
            }
        }
    }
    if marks.is_empty() {
        return Err(error("No Codex proposals matched exact quotations"));
    }
    append_layer(handoff, &mut reading, identity, layer, marks, source_pages).map_err(error)?;
    Ok(reading)
}

fn task(brackets: bool, budget: usize) -> String {
    if brackets {
        "Pass 1. Add Import brackets for each substantive paragraph or coherent unit. Resolve backward references from supplied material, label inferred premises Interpretation, and add separate Export brackets for conclusions. Each bracket needs a short title, stable argumentId, attribute-sensitive context, and an exact quote spanning the whole unit. Index different units separately. Use import or export as category. Do not highlight.".into()
    } else {
        format!(
            "Pass 2. Use the supplied bracket context and propose highlights for exact relevant passages. Preserve all qualifications. Apply at most {budget} words of highlights in this batch. Use central for theses or indispensable premises, supporting for examples or reasons, definition for distinctions, question for explicit questions, or unclear for unresolved passages. Leave title and argumentId empty. Do not add brackets."
        )
    }
}

fn schema(brackets: bool) -> Value {
    let categories = if brackets { vec!["import", "export"] } else { vec!["central", "supporting", "definition", "question", "unclear"] };
    json!({"type":"object","additionalProperties":false,"required":["marks"],"properties":{"marks":{"type":"array","items":{
        "type":"object","additionalProperties":false,"required":["page","quote","category","title","argumentId","comment"],
        "properties":{"page":{"type":"integer"},"quote":{"type":"string"},"category":{"type":"string","enum":categories},
            "title":{"type":"string"},"argumentId":{"type":"string"},"comment":{"type":"string"}}
    }}}})
}

fn load_checkpoint(path: &Path) -> Result<Option<Vec<u8>>, ReadingError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take((MAX_CHECKPOINT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CHECKPOINT {
        return Err(ReadingError::TooLarge(MAX_CHECKPOINT));
    }
    Ok(Some(bytes))
}

fn error(error: impl std::fmt::Display) -> ReadingError {
    ReadingError::Operation(error.to_string())
}
