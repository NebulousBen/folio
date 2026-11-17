use std::sync::Arc;

use pdfcraft_cos::Dict;
use pdfcraft_folio_reading::library::handoff::{Handoff, MAX_PAGES, Page};
use pdfcraft_render::{PageInfo, PageRenderer, PageText, RenderConfig, RenderRequest, text::TextFilter};

use super::{
    Annotation, AnnotationKind, CATALOG_KEY, Category, CategoryKind, DocumentKey, Layer, LayerKind, PdfDocument, Reading, SectionRole, pdf_error,
    reading_error,
};
use crate::{DocId, EditError, Session};

pub mod connected;

const PALETTE: [(&str, &str, &str, CategoryKind); 7] = [
    ("central", "Central / key claim", "#bd9be8", CategoryKind::Highlight),
    ("supporting", "Supporting / relevant", "#f4cf65", CategoryKind::Highlight),
    ("definition", "Definition / distinction", "#91c6a5", CategoryKind::Highlight),
    ("question", "Question", "#e7a1b3", CategoryKind::Highlight),
    ("unclear", "Unclear / unresolved", "#8fbce5", CategoryKind::Highlight),
    ("import", "Imports", "#63876f", CategoryKind::Section),
    ("export", "Exports", "#567da3", CategoryKind::Section),
];

impl Session {
    pub fn folio_handoff_excerpt(&self, id: DocId, pages: &[u32]) -> Result<(Arc<Vec<u8>>, Vec<Page>), EditError> {
        let source = self.get(id).ok_or(EditError::NoDocument)?;
        source.check_copying()?;
        if source.dirty {
            return Err(invalid("Save the PDF before preparing an AI reading"));
        }
        if pages.is_empty() || pages.len() > MAX_PAGES || pages[0] == 0 || pages.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(invalid("Select physical pages in document order"));
        }
        let mut mappings = Vec::with_capacity(pages.len());
        let mut indices = Vec::with_capacity(pages.len());
        for &source_page in pages {
            let index = (source_page - 1) as usize;
            let info = source.info.pages.get(index).ok_or_else(|| invalid("A selected page is outside the PDF"))?;
            mappings.push(Page { source_page, label: info.label.clone() });
            indices.push(index);
        }
        let mut pdf = PdfDocument::open(self.extract(id, &indices)?).map_err(pdf_error)?;
        let root = pdf.root().ok_or_else(|| invalid("The selected PDF has no catalog"))?;
        pdf.update_dict(root, |dict| {
            for key in [CATALOG_KEY, "Outlines", "AcroForm", "Names", "AF", "Metadata", "AA", "OpenAction", "Collection"] {
                dict.remove(key.as_bytes());
            }
        })
        .map_err(pdf_error)?;
        for page in pdfcraft_annot::page_refs(&pdf).map_err(pdf_error)? {
            pdf.update_dict(page, |dict| {
                for key in [b"Annots".as_slice(), b"Metadata", b"AF", b"AA", b"PieceInfo"] {
                    dict.remove(key);
                }
            })
            .map_err(pdf_error)?;
        }
        Ok((self.write_new(&pdf)?, mappings))
    }
}

/// Import only validated annotations. The result PDF's bytes are never copied into the source.
pub fn import_result(
    handoff: &Handoff,
    excerpt: Arc<Vec<u8>>,
    result: Arc<Vec<u8>>,
    source_page_count: u32,
    mut reading: Reading,
) -> Result<Reading, EditError> {
    if DocumentKey::of(&excerpt) != handoff.excerpt_key {
        return Err(invalid("The supplied handoff excerpt changed"));
    }
    let result_key = DocumentKey::of(&result);
    if reading.layers.iter().any(|layer| layer.result_hash.as_ref() == Some(&result_key)) {
        return Ok(reading);
    }
    let pdf = PdfDocument::open(result.clone()).map_err(pdf_error)?;
    let baseline = pdfcraft_render::inspect(excerpt.clone(), None).map_err(pdf_error)?;
    let returned = pdfcraft_render::inspect(result.clone(), None).map_err(pdf_error)?;
    let pages = pdfcraft_annot::page_refs(&pdf).map_err(pdf_error)?;
    if pages.len() != handoff.pages.len() || baseline.pages.len() != pages.len() || returned.pages.len() != pages.len() {
        return Err(invalid("The result PDF has a different page count from the handoff"));
    }
    let config = RenderConfig { hide_comments: true, ..RenderConfig::default() };
    let mut before = PageRenderer::new(excerpt, config.clone());
    let mut after = PageRenderer::new(result, config);
    let created = utc_now();
    let layer_id = format!("folio-ai-{}", result_key.as_str());
    let mut marks = Vec::new();
    for (index, reference) in pages.into_iter().enumerate() {
        let mapping = &handoff.pages[index];
        let info = &baseline.pages[index];
        let actual = &returned.pages[index];
        if !same_geometry(info, actual) || mapping.source_page == 0 || mapping.source_page > source_page_count {
            return Err(invalid(format!("Page {} does not match the handoff page geometry", index + 1)));
        }
        let req = RenderRequest { page: index, scale: 0.75, ..RenderRequest::default() };
        let clean = before.render(req);
        let annotated = after.render(req);
        if clean.error.is_some()
            || annotated.error.is_some()
            || clean.width != annotated.width
            || clean.height != annotated.height
            || clean.rgba.as_ref() != annotated.rgba.as_ref()
        {
            return Err(invalid(format!("Underlying page content differs on handoff page {}", index + 1)));
        }
        let text = before.filtered_text(index, TextFilter::default()).map_err(pdf_error)?;
        let result_text = after.filtered_text(index, TextFilter::default()).map_err(pdf_error)?;
        if text.text_of(0..text.glyphs.len()) != result_text.text_of(0..result_text.glyphs.len()) {
            return Err(invalid(format!("Underlying text differs on handoff page {}", index + 1)));
        }
        let page = pdf.get(reference);
        let Some(annots) = page.as_dict().and_then(|dict| dict.get(b"Annots")) else { continue };
        let annots = pdf.resolve(annots);
        let Some(annots) = annots.as_array() else { continue };
        for entry in annots {
            let entry = pdf.resolve(entry);
            let Some(dict) = entry.as_dict() else { continue };
            let id = format!("{layer_id}-{}", marks.len() + 1);
            if let Some(mark) = read_mark(&pdf, dict, info, &text, mapping.source_page, &layer_id, id, &created)? {
                marks.push(mark);
            }
            if marks.len() > 10_000 {
                return Err(invalid("The result contains too many annotations"));
            }
        }
    }
    if marks.is_empty() {
        return Err(invalid("The result has no Folio brackets or highlights"));
    }
    append_layer(handoff, &mut reading, result_key, layer_id, marks, source_page_count)?;
    Ok(reading)
}

fn append_layer(
    handoff: &Handoff,
    reading: &mut Reading,
    result_key: DocumentKey,
    layer_id: String,
    marks: Vec<Annotation>,
    source_page_count: u32,
) -> Result<(), EditError> {
    let legend: Vec<_> = PALETTE
        .iter()
        .map(|&(id, label, hex, kind)| Category {
            id: format!("{layer_id}-{id}"),
            label: label.into(),
            hex: hex.into(),
            archived: None,
            meaning: None,
            kind: Some(kind),
        })
        .collect();
    reading.layers.push(Layer {
        id: layer_id,
        title: handoff.name.clone(),
        author: "AI".into(),
        visible: true,
        kind: LayerKind::Ai,
        run_id: Some(handoff.excerpt_key.as_str().into()),
        import_key: Some(result_key.as_str().into()),
        color_ids: legend.iter().map(|category| category.id.clone()).collect(),
        created_at: Some(utc_now()),
        purpose: Some(handoff.purpose.clone()),
        standard_version: Some(2),
        legend: legend.clone(),
        result_hash: Some(result_key),
        source_key: Some(handoff.source_key.clone()),
    });
    reading.highlight_colors.extend(legend);
    reading.annotations.extend(marks);
    reading.validate(Some(source_page_count)).map_err(reading_error)
}

#[expect(clippy::too_many_arguments, reason = "The mark keeps its original page, report identity and measured geometry")]
fn read_mark(
    pdf: &PdfDocument,
    dict: &Dict,
    geometry: &PageInfo,
    text: &PageText,
    page: u32,
    layer: &str,
    id: String,
    created: &str,
) -> Result<Option<Annotation>, EditError> {
    let value = |key: &[u8]| dict.get(key).map(|value| pdf.resolve(value));
    let string = |key: &[u8]| value(key).and_then(|v| v.as_string().map(pdfcraft_cos::PdfString::to_text)).unwrap_or_default();
    let subject = string(b"Subj");
    let mut contents = string(b"Contents");
    let (kind, title, argument_id, section_role, category, quads) = match dict.name(b"Subtype") {
        Some(b"Highlight") => {
            let Some((category, ..)) = PALETTE.iter().take(5).find(|row| row.1.eq_ignore_ascii_case(subject.trim())) else {
                return Err(invalid(format!("Highlight has an unknown category on source page {page}")));
            };
            let quads = value(b"QuadPoints")
                .and_then(|v| {
                    let values: Vec<f64> = v.as_array()?.iter().map(|p| pdf.resolve(p).as_f64()).collect::<Option<_>>()?;
                    values.chunks(8).map(|q| <[f64; 8]>::try_from(q).ok()).collect()
                })
                .ok_or_else(|| invalid(format!("Missing highlight quadrilaterals on source page {page}")))?;
            (AnnotationKind::Highlight, None, None, None, *category, quads)
        }
        Some(b"Square") if subject.starts_with("Folio bracket: ") => {
            let title = subject.trim_start_matches("Folio bracket: ").trim();
            let fields: Vec<_> = title.splitn(3, " | ").collect();
            let [argument, role, label] = fields.as_slice() else { return Err(invalid("Invalid Folio bracket title")) };
            if arguments_invalid(argument) || label.trim().is_empty() || title.len() > 240 {
                return Err(invalid("Invalid Folio bracket title"));
            }
            let (role, category) = match *role {
                "Import" => (SectionRole::Import, "import"),
                "Export" => (SectionRole::Export, "export"),
                _ => return Err(invalid("Folio brackets must be Import or Export")),
            };
            let quads = serde_json::from_str(&string(b"FolioSectionQuads")).map_err(pdf_error)?;
            if let Some(body) = contents.strip_prefix(title) {
                contents = body.trim_start().into();
            }
            (AnnotationKind::Section, Some((*label).into()), Some((*argument).into()), Some(role), category, quads)
        }
        _ => return Ok(None),
    };
    let quote = measure(&quads, geometry, text, page)?;
    Ok(Some(Annotation {
        id,
        page,
        kind,
        color: format!("{layer}-{category}"),
        quads,
        quote,
        comment: contents,
        created_at: created.into(),
        title,
        argument_id,
        section_role,
        source_id: None,
        source_name: None,
        layer_id: Some(layer.into()),
        author: Some("AI".into()),
        ai: None,
    }))
}

fn measure(quads: &[[f64; 8]], geometry: &PageInfo, text: &PageText, page: u32) -> Result<String, EditError> {
    if quads.is_empty() || quads.len() > 500 {
        return Err(invalid(format!("Invalid annotation geometry on source page {page}")));
    }
    let mut hits = std::collections::BTreeSet::new();
    for quad in quads {
        let mut rect = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for point in quad.chunks_exact(2) {
            let [x, y] = geometry.user_to_view(point[0] as f32, point[1] as f32);
            if !x.is_finite() || !y.is_finite() || !(-1.0..=geometry.width + 1.0).contains(&x) || !(-1.0..=geometry.height + 1.0).contains(&y) {
                return Err(invalid(format!("An annotation falls outside source page {page}")));
            }
            rect = [rect[0].min(x), rect[1].min(y), rect[2].max(x), rect[3].max(y)];
        }
        if rect[2] - rect[0] < 0.1 || rect[3] - rect[1] < 0.1 {
            return Err(invalid(format!("An annotation has an empty text anchor on source page {page}")));
        }
        hits.extend(text.glyphs_in(rect));
    }
    let quote = text.column_text(&hits.into_iter().collect::<Vec<_>>());
    if quote.trim().is_empty() && !text.glyphs.is_empty() {
        return Err(invalid(format!("An annotation does not cover text on source page {page}")));
    }
    Ok(quote)
}

fn arguments_invalid(argument: &str) -> bool {
    argument.len() > 100
        || argument.bytes().next().is_none_or(|b| !b.is_ascii_alphabetic())
        || argument.bytes().any(|b| !b.is_ascii_alphanumeric() && !matches!(b, b'-' | b'_' | b'.'))
}

fn same_geometry(before: &PageInfo, after: &PageInfo) -> bool {
    before.rotation == after.rotation
        && (before.width - after.width).abs() < 0.01
        && (before.height - after.height).abs() < 0.01
        && before.crop.iter().zip(after.crop).all(|(&left, right)| (left - right).abs() < 0.01)
}

fn utc_now() -> String {
    chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).to_rfc3339()
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}
