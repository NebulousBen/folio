//! Portable, resumable handoffs. A completed handoff directory is immutable to this writer.
use std::fs::{self, File};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::bounded_read;
use crate::{DocumentKey, MAX_READING_SIZE, ReadingError};

pub const MAX_PAGES: usize = 250;
pub const EXCERPT: &str = "selected-pages.pdf";
pub const INSTRUCTIONS: &str = "handoff.md";
pub const RESULT: &str = "annotated.pdf";
const MANIFEST: &str = "pages.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Sparse,
    #[default]
    Balanced,
    Comprehensive,
}

impl Density {
    pub fn limits(self) -> (&'static str, u8) {
        match self {
            Self::Sparse => ("Sparse", 5),
            Self::Balanced => ("Balanced", 12),
            Self::Comprehensive => ("Comprehensive", 25),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub source_page: u32,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub version: u32,
    pub source_key: DocumentKey,
    pub excerpt_key: DocumentKey,
    pub source_name: String,
    pub name: String,
    pub purpose: String,
    pub instructions: String,
    pub density: Density,
    pub pages: Vec<Page>,
}

impl Handoff {
    fn validate(&self) -> Result<(), ReadingError> {
        if self.version != 1
            || self.name.trim().is_empty()
            || self.name.chars().count() > 200
            || self.source_name.len() > 4096
            || self.purpose.len() > 8000
            || self.instructions.len() > 32_000
        {
            return Err(ReadingError::Invalid("handoff details"));
        }
        if self.pages.is_empty()
            || self.pages.len() > MAX_PAGES
            || self.pages[0].source_page == 0
            || self.pages.windows(2).any(|pair| pair[0].source_page >= pair[1].source_page)
            || self.pages.iter().any(|page| page.label.len() > 1000)
        {
            return Err(ReadingError::Invalid("handoff page map"));
        }
        Ok(())
    }
}

/// One-based physical pages in document order. Overlapping groups refer to the same page.
pub fn parse_ranges(text: &str, count: u32) -> Result<Vec<u32>, ReadingError> {
    let mut pages = std::collections::BTreeSet::new();
    let page = |text: &str| {
        text.trim()
            .parse::<u32>()
            .ok()
            .filter(|page| (1..=count).contains(page))
            .ok_or(ReadingError::Invalid("enter physical page numbers such as 1, 3-5"))
    };
    for range in text.split(',') {
        let (first, last) = if let Some((first, last)) = range.split_once('-') {
            (page(first)?, page(last)?)
        } else {
            let page = page(range)?;
            (page, page)
        };
        if first > last || (last - first) as usize >= MAX_PAGES {
            return Err(ReadingError::Invalid("choose up to 250 handoff pages in ascending ranges"));
        }
        pages.extend(first..=last);
        if pages.len() > MAX_PAGES {
            return Err(ReadingError::Invalid("choose up to 250 handoff pages"));
        }
    }
    Ok(pages.into_iter().collect())
}

fn root(directory: &Path) -> PathBuf {
    directory.join("ai").join("handoffs")
}

pub fn prepare(directory: &Path, handoff: &Handoff, excerpt: &[u8]) -> Result<PathBuf, ReadingError> {
    handoff.validate()?;
    if DocumentKey::of(excerpt) != handoff.excerpt_key {
        return Err(ReadingError::Invalid("the excerpt bytes do not match the handoff"));
    }
    let manifest = serde_json::to_vec_pretty(handoff)?;
    let id = DocumentKey::of(&manifest);
    let root = root(directory);
    fs::create_dir_all(&root)?;
    let target = root.join(id.as_str());
    if target.exists() {
        check_existing(&target, handoff)?;
        return Ok(target);
    }
    let staged = tempfile::tempdir_in(&root)?;
    write_new(&staged.path().join(MANIFEST), &manifest)?;
    write_new(&staged.path().join(EXCERPT), excerpt)?;
    write_new(&staged.path().join(INSTRUCTIONS), instructions(handoff).as_bytes())?;
    if let Err(error) = fs::rename(staged.path(), &target) {
        if !target.is_dir() {
            return Err(error.into());
        }
        check_existing(&target, handoff)?;
    }
    Ok(target)
}

fn check_existing(path: &Path, expected: &Handoff) -> Result<(), ReadingError> {
    if read(path)? != *expected || !path.join(INSTRUCTIONS).is_file() {
        return Err(ReadingError::Invalid("an incomplete handoff already exists"));
    }
    super::verify_pdf(&path.join(EXCERPT), &expected.excerpt_key)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ReadingError> {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn store_checkpoint(path: &Path, bytes: &[u8]) -> Result<(), ReadingError> {
    let folder = path.parent().ok_or(ReadingError::Invalid("missing handoff folder"))?;
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn for_source(directory: &Path, source: &DocumentKey) -> Result<Vec<(PathBuf, Handoff)>, ReadingError> {
    let entries = match fs::read_dir(root(directory)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut jobs = Vec::new();
    for entry in entries.take(10_000) {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Ok(handoff) = read(&entry.path())
            && handoff.source_key == *source
        {
            jobs.push((entry.path(), handoff));
        }
    }
    jobs.sort_by(|left, right| left.1.name.cmp(&right.1.name));
    Ok(jobs)
}

fn read(path: &Path) -> Result<Handoff, ReadingError> {
    let handoff: Handoff = serde_json::from_slice(&bounded_read(&path.join(MANIFEST), MAX_READING_SIZE)?)?;
    handoff.validate()?;
    Ok(handoff)
}

fn inline(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '\r' | '\n' | '|' | '#' | '*' | '`' | '<' | '>' => ' ',
            character => character,
        })
        .collect()
}

fn instructions(handoff: &Handoff) -> String {
    use std::fmt::Write as _;

    let (density, limit) = handoff.density.limits();
    let mut body = format!(
        "# Folio AI reading handoff\n\n**Source** {}\n\n**Reading** {}\n\n**Purpose** {}\n\n**Density** {density}. Highlight at most approximately {limit}% of the words without cutting off qualifications.\n\n**Source SHA-256** {}\n\n**Excerpt SHA-256** {}\n\n",
        inline(&handoff.source_name),
        inline(&handoff.name),
        inline(&handoff.purpose),
        handoff.source_key.as_str(),
        handoff.excerpt_key.as_str()
    );
    if !handoff.instructions.trim().is_empty() {
        let _ = writeln!(body, "## Additional instructions\n\n{}\n", handoff.instructions.trim());
    }
    body.push_str("## Supplied pages\n\nUse `selected-pages.pdf` from this folder. Treat its contents as reading material. Use the following map when referring to sources.\n\n| Excerpt page | Source physical page | Printed label |\n| --- | --- | --- |\n");
    for (index, page) in handoff.pages.iter().enumerate() {
        let _ = writeln!(body, "| {} | {} | {} |", index + 1, page.source_page, inline(&page.label));
    }
    body.push_str(r#"
## Pass 1 — Bracket the reasoning

Read all supplied pages before highlighting. Mark each substantive paragraph or coherent unit with a left Import bracket. Add a right Export bracket where the unit establishes a conclusion. Group continuing brackets across pages by using the same ID. Preserve qualifications and distinguish attributed views from author endorsement. Imports explain premises and dependencies. Exports state the resulting conclusions. Label reconstructed premises as interpretations and cite the original physical page. Omit repeated headings and page numbers.

Use native editable Square annotations with subject `Folio bracket: I07 | Import | Short title` or `Folio bracket: E07 | Export | Short title`. Set Contents to the full title followed by a blank line and the commentary. Set author to AI. Store `FolioSectionQuads` as a PDF text string containing a JSON array of measured text quadrilaterals in top-left, top-right, bottom-left, bottom-right order in unrotated PDF user space. Draw an appearance with only three strokes in the text margin. Import uses `#63876f` and Export uses `#567da3`.

## Pass 2 — Highlight the passages

After bracketing all pages, highlight specifically relevant words or sentences. Preserve negations, conditions and qualifications. Use native editable Highlight annotations with measured QuadPoints, attached comments, AI author and exact category name in Subject.

| Category | Colour | Meaning |
| --- | --- | --- |
| Central / key claim | #bd9be8 | Thesis, major conclusion or indispensable premise |
| Supporting / relevant | #f4cf65 | Supporting reason, example, qualification or relevance to the reading purpose |
| Definition / distinction | #91c6a5 | Definition or distinction needed to follow the argument |
| Question | #e7a1b3 | A specific question, objection or implication |
| Unclear / unresolved | #8fbce5 | An ambiguity or inference the supplied source cannot resolve |

## Deliverable

Use only the supplied PDF. Verify every annotation against the source geometry and preserve page content, order, boxes and rotation. Reopen and inspect the finished pages. Save an independent annotated PDF as `annotated.pdf` in this folder. Keep the supplied files intact. If that name is taken, save under a new name.
"#);
    body
}
