//! Folio reading data uses one-based page indices and unrotated PDF user-space coordinates.
//! The version-one types are shared by the old JSON backups and embedded PDF reading data.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{BTreeMap, HashSet};
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(not(target_arch = "wasm32"))]
pub mod library;

pub const MAX_READING_SIZE: usize = 20 * 1024 * 1024;
pub const MAX_BACKUP_SIZE: usize = 200 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, Ord, PartialOrd)]
#[serde(try_from = "String", into = "String")]
pub struct DocumentKey(String);

impl DocumentKey {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DocumentKey {
    type Error = ReadingError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64 || value.bytes().any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b)) {
            return Err(ReadingError::Invalid("document key is not a SHA-256 digest"));
        }
        Ok(Self(value))
    }
}

impl From<DocumentKey> for String {
    fn from(key: DocumentKey) -> Self {
        key.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadingError {
    #[error("Invalid reading data: {0}")]
    Invalid(&'static str),
    #[error("Reading data exceeds {0} bytes")]
    TooLarge(usize),
    #[error("Invalid Folio JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Reading library: {0}")]
    Io(#[from] std::io::Error),
    #[error("This reading changed in another window. Reopen the reading before editing it again.")]
    Conflict,
    #[error("{0}")]
    Operation(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub version: u32,
    pub annotations: Vec<Annotation>,
    pub bookmarks: Vec<Bookmark>,
    pub page: u32,
    pub zoom: Zoom,
    pub rotation: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_source_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub highlight_colors: Vec<Category>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_layer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_keys: Vec<DocumentKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection_sources: Vec<SelectionSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub form_values: BTreeMap<String, FormValue>,
}

impl Default for Reading {
    fn default() -> Self {
        Self {
            version: 1,
            annotations: Vec::new(),
            bookmarks: Vec::new(),
            page: 1,
            zoom: Zoom::Fit(Fit::Width),
            rotation: 0,
            position: None,
            managed_source_ids: Vec::new(),
            highlight_colors: Vec::new(),
            layers: Vec::new(),
            active_layer_id: None,
            source_keys: Vec::new(),
            selection_sources: Vec::new(),
            form_values: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Zoom {
    Ratio(f64),
    Fit(Fit),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    Width,
    Page,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    pub id: String,
    pub page: u32,
    pub kind: AnnotationKind,
    pub color: String,
    pub quads: Vec<[f64; 8]>,
    pub quote: String,
    pub comment: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section_role: Option<SectionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai: Option<AiAnnotation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationKind {
    Highlight,
    Note,
    Section,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SectionRole {
    Import,
    Export,
    Argument,
    Setup,
    Objection,
    Reply,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiAnnotation {
    pub run_id: String,
    pub proposal_id: String,
    pub status: String,
    pub secondary_categories: Vec<String>,
    pub topic: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub id: String,
    pub page: u32,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_label: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Category {
    pub id: String,
    pub hex: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meaning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<CategoryKind>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CategoryKind {
    Highlight,
    Section,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: String,
    pub title: String,
    pub author: String,
    pub visible: bool,
    pub kind: LayerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub color_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standard_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legend: Vec<Category>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<DocumentKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_key: Option<DocumentKey>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    Personal,
    Ai,
    Imported,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelectionSource {
    pub key: DocumentKey,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub page_count: u32,
    pub pages: Vec<PageMapping>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageMapping {
    pub page: u32,
    pub source_page: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct FormValue {
    pub value: FieldValue,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum FieldValue {
    Text(String),
    Toggle(bool),
    Multiple(Vec<String>),
}

impl Reading {
    pub fn visible_annotations(&self) -> impl Iterator<Item = &Annotation> {
        let hidden: HashSet<_> = self.layers.iter().filter(|layer| !layer.visible).map(|layer| layer.id.as_str()).collect();
        self.annotations.iter().filter(move |mark| mark.layer_id.as_deref().is_none_or(|id| !hidden.contains(id)))
    }

    pub fn colour(&self, id: &str) -> [u8; 3] {
        let custom = self.highlight_colors.iter().find(|category| category.id == id).and_then(|category| {
            let hex = category.hex.strip_prefix('#')?;
            let value = u32::from_str_radix(hex, 16).ok()?;
            Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
        });
        custom.unwrap_or(match id {
            "green" => [112, 204, 130],
            "blue" => [108, 176, 242],
            "pink" => [238, 135, 168],
            "purple" => [179, 140, 221],
            _ => [255, 213, 79],
        })
    }

    pub fn decode(bytes: &[u8], page_count: Option<u32>) -> Result<Self, ReadingError> {
        check_size(bytes, MAX_READING_SIZE)?;
        let reading: Self = serde_json::from_slice(bytes)?;
        reading.validate(page_count)?;
        Ok(reading)
    }

    pub fn encode(&self, page_count: Option<u32>) -> Result<Vec<u8>, ReadingError> {
        self.validate(page_count)?;
        let bytes = serde_json::to_vec(self)?;
        check_size(&bytes, MAX_READING_SIZE)?;
        Ok(bytes)
    }

    /// Selected bookmark ranges, in document order and with no duplicate pages.
    /// An omitted end runs up to the next distinct bookmark start or to the end.
    pub fn bookmark_ranges(&self, ids: &[&str], page_count: u32) -> Result<Vec<RangeInclusive<u32>>, ReadingError> {
        self.validate(Some(page_count))?;
        let bookmarks: BTreeMap<_, _> = self.bookmarks.iter().map(|bookmark| (bookmark.id.as_str(), bookmark)).collect();
        let mut starts: Vec<_> = self.bookmarks.iter().map(|bookmark| bookmark.page).collect();
        starts.sort_unstable();
        starts.dedup();
        let mut ranges = Vec::new();
        for id in ids {
            let bookmark = bookmarks.get(id).ok_or(ReadingError::Invalid("unknown bookmark"))?;
            let end = bookmark
                .end_page
                .unwrap_or_else(|| starts.get(starts.partition_point(|start| *start <= bookmark.page)).map_or(page_count, |next| next - 1));
            ranges.push((bookmark.page, end));
        }
        ranges.sort_unstable();
        let mut combined: Vec<(u32, u32)> = Vec::new();
        for (start, end) in ranges {
            if let Some(previous) = combined.last_mut().filter(|previous| start <= previous.1.saturating_add(1)) {
                previous.1 = previous.1.max(end);
            } else {
                combined.push((start, end));
            }
        }
        Ok(combined.into_iter().map(|(start, end)| start..=end).collect())
    }

    /// Copy only retained reading data into an excerpt. `map` maps original pages to new pages.
    pub fn excerpt(&self, source: SelectionSource, map: &BTreeMap<u32, u32>) -> Self {
        let mut reading = self.clone();
        reading.annotations.retain_mut(|annotation| {
            let Some(page) = map.get(&annotation.page) else { return false };
            annotation.page = *page;
            true
        });
        reading.bookmarks.retain_mut(|bookmark| {
            let Some(page) = map.get(&bookmark.page) else { return false };
            bookmark.page = *page;
            bookmark.end_page = bookmark.end_page.and_then(|end| map.get(&end).copied());
            true
        });
        reading.selection_sources.retain_mut(|existing| {
            existing.pages.retain_mut(|mapping| {
                let Some(page) = map.get(&mapping.page) else { return false };
                mapping.page = *page;
                true
            });
            !existing.pages.is_empty()
        });
        let already_mapped: HashSet<_> = reading.selection_sources.iter().flat_map(|entry| entry.pages.iter().map(|p| p.page)).collect();
        let mut source = source;
        source.pages.retain(|page| !already_mapped.contains(&page.page));
        if !source.pages.is_empty() {
            if let Some(existing) = reading.selection_sources.iter_mut().find(|existing| existing.key == source.key) {
                existing.pages.extend(source.pages);
            } else {
                reading.selection_sources.push(source);
            }
        }
        reading.page = 1;
        reading.position = None;
        reading.form_values.clear();
        reading
    }

    /// Retain anchors only on pages whose content and bounds are known to survive a revision.
    /// An explicit range is retained only when its pages remain together in the same order.
    pub fn rebased(&self, unchanged: &BTreeMap<u32, u32>) -> Self {
        let mut reading = self.clone();
        reading.annotations.retain_mut(|mark| {
            let Some(&page) = unchanged.get(&mark.page) else { return false };
            mark.page = page;
            true
        });
        reading.bookmarks.retain_mut(|bookmark| {
            let start = bookmark.page;
            let Some(&page) = unchanged.get(&start) else { return false };
            if let Some(end) = bookmark.end_page {
                let expected = end.saturating_sub(start).saturating_add(1);
                let mapped = unchanged.range(start..=end);
                let continuous = mapped.clone().count() == expected as usize && mapped.zip(page..).all(|((_, &page), next)| page == next);
                bookmark.end_page = continuous.then(|| page.saturating_add(expected - 1));
                if !continuous {
                    return false;
                }
            }
            bookmark.page = page;
            true
        });
        reading.selection_sources.retain_mut(|source| {
            source.pages.retain_mut(|mapping| {
                let Some(&page) = unchanged.get(&mapping.page) else { return false };
                mapping.page = page;
                true
            });
            !source.pages.is_empty()
        });
        if let Some(&page) = unchanged.get(&self.page) {
            reading.page = page;
        } else {
            reading.page = 1;
            reading.position = None;
        }
        reading
    }

    pub fn validate(&self, page_count: Option<u32>) -> Result<(), ReadingError> {
        let limit = page_count.unwrap_or(u32::MAX);
        if self.version != 1 || !valid_page(self.page, limit) || !matches!(self.rotation, 0 | 90 | 180 | 270) {
            return Err(ReadingError::Invalid("version, page, or rotation"));
        }
        if matches!(self.zoom, Zoom::Ratio(zoom) if !(0.25..=4.0).contains(&zoom))
            || self.position.is_some_and(|p| !(0.0..=1.0).contains(&p.x) || !(0.0..=1.0).contains(&p.y))
        {
            return Err(ReadingError::Invalid("zoom or position"));
        }
        self.validate_annotations(limit)?;
        self.validate_bookmarks(limit)?;
        self.validate_sources(limit)?;
        Ok(())
    }

    fn validate_annotations(&self, page_count: u32) -> Result<(), ReadingError> {
        let mut categories = category_ids(&self.highlight_colors)?;
        categories.extend(["yellow", "green", "blue", "pink", "purple"]);
        let layers = unique_ids(self.layers.iter().map(|layer| layer.id.as_str()))?;
        if self.active_layer_id.as_deref().is_some_and(|id| !layers.contains(id)) {
            return Err(ReadingError::Invalid("unknown active layer"));
        }
        for layer in &self.layers {
            if layer.title.trim().is_empty()
                || layer.author.trim().is_empty()
                || layer.standard_version == Some(0)
                || layer.color_ids.iter().any(|id| !categories.contains(id.as_str()))
            {
                return Err(ReadingError::Invalid("layer title, author, standard version or categories"));
            }
            category_ids(&layer.legend)?;
        }
        unique_ids(self.annotations.iter().map(|annotation| annotation.id.as_str()))?;
        for mark in &self.annotations {
            if !valid_page(mark.page, page_count)
                || !categories.contains(mark.color.as_str())
                || mark.layer_id.as_deref().is_some_and(|id| !layers.contains(id))
                || mark.quads.is_empty()
                || !mark.quads.iter().all(valid_quad)
            {
                return Err(ReadingError::Invalid("annotation page, category, layer or geometry"));
            }
            match mark.kind {
                AnnotationKind::Section if mark.title.as_deref().is_none_or(|s| s.trim().is_empty()) => {
                    return Err(ReadingError::Invalid("missing bracket title"));
                }
                AnnotationKind::Highlight | AnnotationKind::Note if mark.argument_id.is_some() || mark.section_role.is_some() => {
                    return Err(ReadingError::Invalid("bracket data on a highlight or comment"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_bookmarks(&self, page_count: u32) -> Result<(), ReadingError> {
        unique_ids(self.bookmarks.iter().map(|bookmark| bookmark.id.as_str()))?;
        if self.bookmarks.iter().any(|b| !valid_page(b.page, page_count) || b.end_page.is_some_and(|e| !(b.page..=page_count).contains(&e))) {
            return Err(ReadingError::Invalid("bookmark range"));
        }
        Ok(())
    }

    fn validate_sources(&self, page_count: u32) -> Result<(), ReadingError> {
        let mut keys = HashSet::new();
        let mut pages = HashSet::new();
        for source in &self.selection_sources {
            if !keys.insert(&source.key) || source.pages.is_empty() {
                return Err(ReadingError::Invalid("duplicate or empty selection source"));
            }
            for mapping in &source.pages {
                if !valid_page(mapping.page, page_count) || !valid_page(mapping.source_page, source.page_count) || !pages.insert(mapping.page) {
                    return Err(ReadingError::Invalid("duplicate or invalid selection page mapping"));
                }
            }
        }
        Ok(())
    }
}

fn valid_page(page: u32, limit: u32) -> bool {
    (1..=limit).contains(&page)
}

fn valid_quad(points: &[f64; 8]) -> bool {
    let [x0, y0, x1, y1, x2, y2, x3, y3] = *points;
    points.iter().all(|coordinate| coordinate.is_finite())
        && x0.max(x1).max(x2).max(x3) > x0.min(x1).min(x2).min(x3)
        && y0.max(y1).max(y2).max(y3) > y0.min(y1).min(y2).min(y3)
}

fn unique_ids<'a>(ids: impl Iterator<Item = &'a str>) -> Result<HashSet<&'a str>, ReadingError> {
    let mut unique = HashSet::new();
    for id in ids {
        if id.trim().is_empty() || !unique.insert(id) {
            return Err(ReadingError::Invalid("empty or duplicate identifier"));
        }
    }
    Ok(unique)
}

fn category_ids(categories: &[Category]) -> Result<HashSet<&str>, ReadingError> {
    if categories
        .iter()
        .any(|c| c.label.trim().is_empty() || c.hex.strip_prefix('#').is_none_or(|rgb| rgb.len() != 6 || !rgb.bytes().all(|b| b.is_ascii_hexdigit())))
    {
        return Err(ReadingError::Invalid("category label or color"));
    }
    unique_ids(categories.iter().map(|category| category.id.as_str()))
}

fn check_size(bytes: &[u8], limit: usize) -> Result<(), ReadingError> {
    if bytes.len() > limit { Err(ReadingError::TooLarge(limit)) } else { Ok(()) }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub format: String,
    pub version: u32,
    pub created_at: String,
    pub documents: Vec<BackupDocument>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackupDocument {
    pub key: DocumentKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    pub state: Reading,
}

impl Backup {
    pub fn decode(bytes: &[u8]) -> Result<Self, ReadingError> {
        check_size(bytes, MAX_BACKUP_SIZE)?;
        let backup: Self = serde_json::from_slice(bytes)?;
        if backup.format != "folio-reading-backup" || backup.version != 1 || chrono::DateTime::parse_from_rfc3339(&backup.created_at).is_err() {
            return Err(ReadingError::Invalid("unsupported or malformed backup header"));
        }
        let mut keys = HashSet::new();
        for entry in &backup.documents {
            if !keys.insert(&entry.key) {
                return Err(ReadingError::Invalid("duplicate document in backup"));
            }
            entry.state.encode(entry.page_count)?;
        }
        Ok(backup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn legacy_reading() -> Value {
        json!({
            "version": 1, "page": 2, "rotation": 90, "zoom": "width",
            "position": {"x": 0.2, "y": 0.6},
            "annotations": [{
                "id": "mark-1", "page": 2, "kind": "section", "color": "green",
                "quads": [[10.0, 80.0, 60.0, 80.0, 10.0, 60.0, 60.0, 60.0]],
                "quote": "Ἀρχή", "comment": "Implicit premise",
                "title": "The starting point", "createdAt": "2026-10-01T15:04:00Z",
                "argumentId": "I01", "sectionRole": "import", "layerId": "ai"
            }],
            "bookmarks": [{"id": "book-1", "page": 2, "endPage": 3, "label": "Book II", "showLabel": true}],
            "layers": [{"id": "ai", "title": "Annotation run", "author": "Test reader", "visible": false, "kind": "ai"}],
            "activeLayerId": "ai",
            "formValues": {"field": {"value": ["First", "Second"]}},
            "selectionSources": [{
                "key": DocumentKey::of(b"source"), "name": "source.pdf", "pageCount": 80,
                "pages": [{"page": 2, "sourcePage": 21}]
            }]
        })
    }

    fn read(value: &Value) -> Result<Reading, ReadingError> {
        Reading::decode(&serde_json::to_vec(value).unwrap(), Some(3))
    }

    #[test]
    fn legacy_layers_brackets_ranges_and_source_pages_survive_round_trip() {
        let state = legacy_reading();
        let reading = read(&state).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&reading.encode(Some(3)).unwrap()).unwrap(), state);
        assert_eq!(DocumentKey::of(b"abc").as_str(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn bad_geometry_references_and_ranges_are_rejected() {
        for (pointer, bad) in [
            ("/annotations/0/quads/0", json!([10, 80, 10, 80, 10, 60, 10, 60])),
            ("/annotations/0/layerId", json!("missing-layer")),
            ("/bookmarks/0/endPage", json!(4)),
            ("/selectionSources/0/pages/0/sourcePage", json!(0)),
        ] {
            let mut state = legacy_reading();
            *state.pointer_mut(pointer).unwrap() = bad;
            assert!(read(&state).is_err(), "accepted invalid data at {pointer}");
        }
    }

    #[test]
    fn backup_cannot_assign_two_records_to_the_same_document() {
        let entry = json!({"key": DocumentKey::of(b"pdf"), "pageCount": 3, "state": legacy_reading()});
        let mut backup = json!({"format": "folio-reading-backup", "version": 1, "createdAt": "2026-10-01T15:00:00Z", "documents": [entry]});
        assert_eq!(Backup::decode(&serde_json::to_vec(&backup).unwrap()).unwrap().documents.len(), 1);
        let docs = backup["documents"].as_array_mut().unwrap();
        docs.push(docs.first().unwrap().clone());
        assert!(Backup::decode(&serde_json::to_vec(&backup).unwrap()).is_err());
    }

    #[test]
    fn bookmarked_passages_use_explicit_ends_and_merge_overlaps() {
        let mut reading = read(&legacy_reading()).unwrap();
        for (id, page) in [("first", 1), ("second", 2), ("last", 3)] {
            reading.bookmarks.push(Bookmark { id: id.into(), page, label: id.into(), end_page: None, show_label: None });
        }
        assert_eq!(reading.bookmark_ranges(&["second"], 4).unwrap(), [2..=2]);
        assert_eq!(reading.bookmark_ranges(&["book-1", "last", "first"], 4).unwrap(), [1..=4]);
        assert!(reading.bookmark_ranges(&["missing"], 4).is_err());
    }

    #[test]
    fn excerpt_retains_only_selected_marks_and_tracks_the_original_source() {
        let reading = read(&legacy_reading()).unwrap();
        let source = SelectionSource {
            name: "Selection.pdf".into(),
            key: DocumentKey::of(b"selection"),
            page_count: 3,
            path: None,
            pages: vec![PageMapping { page: 1, source_page: 2 }, PageMapping { page: 2, source_page: 3 }],
        };
        let excerpt = reading.excerpt(source, &BTreeMap::from([(2, 1), (3, 2)]));
        excerpt.validate(Some(2)).unwrap();
        assert_eq!(excerpt.annotations[0].page, 1);
        assert_eq!(excerpt.bookmarks[0].end_page, Some(2));
        assert_eq!(excerpt.selection_sources[0].pages, [PageMapping { page: 1, source_page: 21 }]);
        assert_eq!(excerpt.selection_sources[1].pages, [PageMapping { page: 2, source_page: 3 }]);
    }

    #[test]
    fn revision_only_transfers_marks_on_retained_pages_and_contiguous_bookmarks() {
        let reading = read(&legacy_reading()).unwrap();
        let moved = reading.rebased(&BTreeMap::from([(2, 1), (3, 2)]));
        moved.validate(Some(3)).unwrap();
        assert_eq!(moved.annotations[0].page, 1);
        assert_eq!((moved.bookmarks[0].page, moved.bookmarks[0].end_page), (1, Some(2)));
        assert_eq!(moved.selection_sources[0].pages, [PageMapping { page: 1, source_page: 21 }]);

        let edited = reading.rebased(&BTreeMap::from([(1, 1), (3, 3)]));
        assert!(edited.annotations.is_empty());
        assert!(edited.bookmarks.is_empty());
        assert!(edited.selection_sources.is_empty());
        assert!(edited.position.is_none());

        let reordered = reading.rebased(&BTreeMap::from([(2, 3), (3, 1)]));
        assert_eq!(reordered.annotations[0].page, 3);
        assert!(reordered.bookmarks.is_empty());
    }
}
