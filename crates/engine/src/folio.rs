//! Read the library metadata embedded in Folio annotated PDFs.

use pdfcraft_folio_reading::MAX_READING_SIZE;
use pdfcraft_folio_reading::PageMapping;
#[cfg(not(target_arch = "wasm32"))]
pub use pdfcraft_folio_reading::library;
pub use pdfcraft_folio_reading::{
    Annotation, AnnotationKind, BackupDocument, Bookmark, Category, CategoryKind, DocumentKey, Fit, Layer, LayerKind, Reading, ReadingError,
    SectionRole, SelectionSource, Zoom,
};

use crate::{DocId, Document, EditError, Session};
use pdfcraft_annot::{Markup, Meta, NewAnnotation, NoteIcon, Shape, Style};
use pdfcraft_cos::{Document as PdfDocument, PdfString};

mod forms;
pub use forms::FormRestore;

#[cfg(not(target_arch = "wasm32"))]
pub mod handoff;
#[cfg(not(target_arch = "wasm32"))]
mod revision;

const CATALOG_KEY: &str = "FolioReadingData";
pub const VERSION: &str = match option_env!("FOLIO_BUILD_VERSION") {
    Some(version) => version,
    None => "0.5.0-dev",
};

impl Document {
    /// Read Folio's portable notes using the same unlocked object graph as the rest of the engine.
    /// This leaves the source and its signature bytes alone.
    pub fn folio_reading(&self) -> Result<Option<Reading>, EditError> {
        self.check_copying()?;
        let page_count = u32::try_from(self.info.pages.len()).map_err(|_| EditError::Invalid("too many pages".into()))?;
        if let Some(editor) = self.editor.as_ref() {
            return read_catalog(&editor.cos, page_count).map_err(reading_error);
        }
        let pdf =
            PdfDocument::open_with_password(self.bytes.clone(), self.password.as_deref()).map_err(|error| EditError::Invalid(error.to_string()))?;
        read_catalog(&pdf, page_count).map_err(reading_error)
    }

    pub fn folio_key(&self) -> DocumentKey {
        DocumentKey::of(&self.saved_bytes())
    }
}

impl Session {
    /// Materialize local marks in a portable PDF copy, leaving the editor and source intact.
    pub fn folio_annotated_copy(&self, id: DocId, reading: &Reading) -> Result<std::sync::Arc<Vec<u8>>, EditError> {
        let source = self.get(id).ok_or(EditError::NoDocument)?;
        source.check_copying()?;
        if source.dirty {
            return Err(EditError::Invalid("Save the PDF before exporting the reading".into()));
        }
        let mut pdf = if let Some(editor) = &source.editor {
            editor.cos.clone()
        } else {
            PdfDocument::open_with_password(source.bytes.clone(), source.password.as_deref()).map_err(pdf_error)?
        };
        materialize(&mut pdf, reading, &source.info.pages.iter().collect::<Vec<_>>())?;
        self.write_new(&pdf)
    }

    /// Build a printable report from the selected Folio notes.
    pub fn folio_notes_report(&self, id: DocId, title: &str, contents: &str) -> Result<std::sync::Arc<Vec<u8>>, EditError> {
        self.get(id).ok_or(EditError::NoDocument)?.check_copying()?;
        self.create_report(title, contents)
    }

    /// Export the selected bookmark ranges using PDFCraft's normal page extractor.
    pub fn extract_folio_bookmarks(&self, id: DocId, reading: &Reading, labels: &[&str]) -> Result<std::sync::Arc<Vec<u8>>, EditError> {
        let source = self.get(id).ok_or(EditError::NoDocument)?;
        if source.dirty {
            return Err(EditError::Invalid("Save the PDF before extracting bookmark pages".into()));
        }
        let page_count = u32::try_from(source.info.pages.len()).map_err(|_| EditError::Invalid("too many pages".into()))?;
        let ranges = reading.bookmark_ranges(labels, page_count).map_err(reading_error)?;
        let page_map: std::collections::BTreeMap<_, _> = ranges.into_iter().flatten().zip(1..).collect();
        if page_map.is_empty() {
            return Err(EditError::Invalid("Select at least one bookmark range".into()));
        }
        let pages: Vec<_> = page_map.keys().map(|page| page.saturating_sub(1) as usize).collect();
        let raw = self.extract(id, &pages)?;
        let copy = SelectionSource {
            key: source.folio_key(),
            name: source.name.clone(),
            path: source.path.clone(),
            page_count,
            pages: page_map.iter().map(|(&source_page, &page)| PageMapping { page, source_page }).collect(),
        };
        let excerpt = reading.excerpt(copy, &page_map);
        let mut pdf = PdfDocument::open(raw).map_err(pdf_error)?;
        let selected_info: Vec<_> = pages.iter().filter_map(|&index| source.info.pages.get(index)).collect();
        materialize(&mut pdf, &excerpt, &selected_info)?;
        self.write_new(&pdf)
    }
}

fn materialize(pdf: &mut PdfDocument, reading: &Reading, pages: &[&pdfcraft_render::PageInfo]) -> Result<(), EditError> {
    let mut portable = reading.clone();
    portable.form_values.clear();
    for source in &mut portable.selection_sources {
        source.path = None;
    }
    let page_count = u32::try_from(pages.len()).map_err(|_| EditError::Invalid("too many pages".into()))?;
    let json = portable.encode(Some(page_count)).map_err(reading_error)?;
    let json = std::str::from_utf8(&json).map_err(pdf_error)?;
    remove_materialized_marks(pdf)?;
    for mark in portable.visible_annotations() {
        let index = (mark.page - 1) as usize;
        let Some(page) = pages.get(index) else { continue };
        let shape = match mark.kind {
            AnnotationKind::Highlight => Shape::TextMarkup { kind: Markup::Highlight, quads: mark.quads.clone() },
            AnnotationKind::Note => Shape::Note { at: [mark.quads[0][0], mark.quads[0][1]], icon: NoteIcon::Comment },
            AnnotationKind::Section => Shape::Ink { strokes: vec![bracket_points(mark, page)] },
        };
        let author = mark
            .author
            .as_deref()
            .or_else(|| portable.layers.iter().find(|layer| mark.layer_id.as_deref() == Some(&layer.id)).map(|layer| layer.author.as_str()));
        let contents =
            [mark.title.as_deref().unwrap_or_default(), &mark.comment].into_iter().filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n");
        let style = Style {
            color: portable.colour(&mark.color).map(|channel| f64::from(channel) / 255.0),
            opacity: if mark.kind == AnnotationKind::Highlight { 0.4 } else { 1.0 },
            width: 1.5,
            ..Style::default()
        };
        let new = NewAnnotation { page: index, shape, style, contents, author: author.unwrap_or("Folio").to_owned() };
        let meta = Meta { id: format!("folio:{}", mark.id), ..Meta::default() };
        pdfcraft_annot::add_annotation(pdf, &new, &meta).map_err(pdf_error)?;
    }
    let root = pdf.root().ok_or_else(|| EditError::Write("PDF has no catalog".into()))?;
    pdf.update_dict(root, |dict| dict.set(CATALOG_KEY, PdfString::text(json))).map_err(pdf_error)?;
    Ok(())
}

fn remove_materialized_marks(pdf: &mut PdfDocument) -> Result<(), EditError> {
    for page in pdfcraft_annot::page_refs(pdf).map_err(pdf_error)? {
        let object = pdf.get(page);
        let Some(annots) = object.as_dict().and_then(|dict| dict.get(b"Annots")) else { continue };
        let annots = pdf.resolve(annots);
        let Some(annots) = annots.as_array() else { continue };
        let retained: Vec<_> = annots
            .iter()
            .filter(|&entry| {
                let entry = pdf.resolve(entry);
                !entry.as_dict().is_some_and(|dict| {
                    matches!(dict.name(b"Subtype"), Some(b"Highlight" | b"Square" | b"Text" | b"Ink"))
                        && dict
                            .get(b"NM")
                            .and_then(|name| pdf.resolve(name).as_string().map(PdfString::to_text))
                            .is_some_and(|name| name.starts_with("folio:"))
                })
            })
            .cloned()
            .collect();
        if retained.len() != annots.len() {
            pdfcraft_annot::set_annots(pdf, page, retained).map_err(pdf_error)?;
        }
    }
    Ok(())
}

/// Draw the bracket beside its selected passage in the page's natural reading orientation.
pub fn bracket_points(mark: &Annotation, page: &pdfcraft_render::PageInfo) -> Vec<[f64; 2]> {
    let mut left = page.width;
    let mut right = 0.0_f32;
    let mut top = page.height;
    let mut bottom = 0.0_f32;
    for point in mark.quads.iter().flat_map(|quad| quad.chunks_exact(2)) {
        let [x, y] = page.user_to_view(point[0] as f32, point[1] as f32);
        left = left.min(x);
        right = right.max(x);
        top = top.min(y);
        bottom = bottom.max(y);
    }
    let (spine, cap) = if mark.section_role == Some(SectionRole::Export) {
        let spine = (right + 9.0).min(page.width);
        (spine, (spine - 6.0).max(0.0))
    } else {
        let spine = (left - 9.0).max(0.0);
        (spine, (spine + 6.0).min(page.width))
    };
    [[cap, top], [spine, top], [spine, bottom], [cap, bottom]].into_iter().map(|[x, y]| page.view_to_user(x, y).map(f64::from)).collect()
}

fn pdf_error(error: impl std::fmt::Display) -> EditError {
    EditError::Write(error.to_string())
}

fn reading_error(error: ReadingError) -> EditError {
    EditError::Invalid(error.to_string())
}

fn read_catalog(pdf: &PdfDocument, pages: u32) -> Result<Option<Reading>, ReadingError> {
    let Some(root) = pdf.root() else { return Ok(None) };
    let root = pdf.get(root);
    let Some(value) = root.as_dict().and_then(|dict| dict.get(CATALOG_KEY.as_bytes())) else { return Ok(None) };
    let value = pdf.resolve(value);
    let value = value.as_string().ok_or(ReadingError::Invalid("FolioReadingData must be a PDF text string"))?;
    // The old writer uses UTF-16BE. Limit encoded bytes before decoding, then apply the
    // regular UTF-8 JSON size limit in Reading::decode.
    let encoded_limit = MAX_READING_SIZE * 2 + 2;
    if value.bytes.len() > encoded_limit {
        return Err(ReadingError::TooLarge(encoded_limit));
    }
    Reading::decode(value.to_text().as_bytes(), Some(pages)).map(Some)
}
