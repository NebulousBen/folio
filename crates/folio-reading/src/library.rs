//! Read and copy the desktop library by content identity. A first import never replaces a reading.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Backup, BackupDocument, DocumentKey, MAX_BACKUP_SIZE, MAX_READING_SIZE, Reading, ReadingError};

pub mod backups;
pub mod handoff;
mod history;
pub mod legacy_ai;
pub mod session;
pub mod workspaces;
pub use history::{Checkpoint, checkpoints};

const MAX_DOCUMENTS: usize = 50_000;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogDocument {
    name: Option<String>,
    path: Option<String>,
    page_count: Option<u32>,
}

#[derive(Default)]
pub struct ImportPreview {
    pub documents: Vec<BackupDocument>,
    pub excluded_markdown: usize,
    pub unreadable: Vec<String>,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub already_present: usize,
    pub conflicts: Vec<DocumentKey>,
}

/// Read a backup or a legacy profile. The caller can review the records before importing them.
pub fn preview(path: &Path) -> Result<ImportPreview, ReadingError> {
    if path.is_dir() {
        return preview_profile(path);
    }
    let backup = Backup::decode(&bounded_read(path, MAX_BACKUP_SIZE)?)?;
    Ok(classify(backup.documents))
}

fn is_markdown(entry: &BackupDocument) -> bool {
    entry
        .path
        .as_deref()
        .or(entry.name.as_deref())
        .and_then(|name| Path::new(name).extension())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

fn classify(documents: Vec<BackupDocument>) -> ImportPreview {
    let (markdown, documents): (Vec<_>, Vec<_>) = documents.into_iter().partition(is_markdown);
    ImportPreview { documents, excluded_markdown: markdown.len(), unreadable: Vec::new() }
}

fn preview_profile(path: &Path) -> Result<ImportPreview, ReadingError> {
    let mut catalog: BTreeMap<DocumentKey, CatalogDocument> = match bounded_read(&path.join("catalog.json"), MAX_READING_SIZE) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(ReadingError::Io(error)) if error.kind() == ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error),
    };
    let mut documents = Vec::new();
    let mut unreadable = Vec::new();
    let mut remaining = MAX_BACKUP_SIZE;
    for (index, entry) in fs::read_dir(path.join("documents"))?.enumerate() {
        if index >= MAX_DOCUMENTS {
            return Err(ReadingError::Invalid("too many documents in the profile"));
        }
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
        let Ok(key) = DocumentKey::try_from(id.to_owned()) else { continue };
        let record = catalog.remove(&key).unwrap_or_default();
        let result = bounded_read(&path, MAX_READING_SIZE).and_then(|bytes| {
            remaining = remaining.checked_sub(bytes.len()).ok_or(ReadingError::TooLarge(MAX_BACKUP_SIZE))?;
            Reading::decode(&bytes, record.page_count)
        });
        match result {
            Ok(state) => documents.push(BackupDocument { key, name: record.name, path: record.path, page_count: record.page_count, state }),
            Err(ReadingError::TooLarge(limit)) if limit == MAX_BACKUP_SIZE => return Err(ReadingError::TooLarge(limit)),
            Err(e) => unreadable.push(format!("{id}: {e}")),
        }
    }
    let mut result = classify(documents);
    result.unreadable = unreadable;
    Ok(result)
}

/// Load exactly the data stored for these PDF bytes. Identical copies can be relocated.
pub fn load(root: &Path, key: &DocumentKey, page_count: u32) -> Result<Option<Reading>, ReadingError> {
    let Some(record) = load_record(root, key)? else { return Ok(None) };
    record.state.validate(Some(page_count))?;
    Ok(Some(record.state))
}

pub fn load_for_revision(root: &Path, key: &DocumentKey) -> Result<Option<Reading>, ReadingError> {
    let Some(record) = load_record(root, key)? else { return Ok(None) };
    record.state.validate(record.page_count)?;
    Ok(Some(record.state))
}

pub fn local_pdf(root: &Path, key: &DocumentKey) -> Result<Option<PathBuf>, ReadingError> {
    Ok(load_record(root, key)?.and_then(|record| record.path).map(PathBuf::from))
}

pub fn verify_pdf(path: &Path, key: &DocumentKey) -> Result<(), ReadingError> {
    use sha2::{Digest as _, Sha256};

    if !path.is_absolute() || path.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("pdf")) {
        return Err(ReadingError::Invalid("the source must be a local PDF file"));
    }
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ReadingError::Invalid("the source must be a local PDF file"));
    }
    let mut buffer = [0_u8; 8192];
    let mut hash = Sha256::new();
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let actual: String = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if actual != key.as_str() {
        return Err(ReadingError::Invalid("this PDF is a different revision from the source recorded in the excerpt"));
    }
    Ok(())
}

fn load_record(root: &Path, key: &DocumentKey) -> Result<Option<BackupDocument>, ReadingError> {
    let file = document_path(root, key);
    let record = match bounded_read(&file, MAX_BACKUP_SIZE) {
        Ok(bytes) => serde_json::from_slice::<BackupDocument>(&bytes)?,
        Err(ReadingError::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if record.key != *key {
        return Err(ReadingError::Invalid("reading file belongs to a different document"));
    }
    Ok(Some(record))
}

/// Replace an autosave only if it still matches the snapshot the editor loaded.
/// The lock is shared between writers for this document, including import.
pub fn save(root: &Path, record: &BackupDocument, expected: Option<&Reading>) -> Result<(), ReadingError> {
    let page_count = record.page_count.unwrap_or(u32::MAX);
    let dir = root.join("documents");
    fs::create_dir_all(&dir)?;
    let _lock = lock_document(&dir, &record.key)?;
    let current = load(root, &record.key, page_count)?;
    if current.as_ref() != expected {
        return Err(ReadingError::Conflict);
    }
    if let Some(previous) = current.filter(|reading| reading != &record.state) {
        history::remember(root, &record.key, previous)?;
    }
    stage(&dir, record)?.persist(document_path(root, &record.key)).map_err(|error| error.error)?;
    Ok(())
}

fn lock_document(dir: &Path, key: &DocumentKey) -> Result<File, ReadingError> {
    let file = File::options().create(true).truncate(false).write(true).open(dir.join(format!("{}.lock", key.as_str())))?;
    file.lock()?;
    Ok(file)
}

fn stage(dir: &Path, record: &BackupDocument) -> Result<tempfile::NamedTempFile, ReadingError> {
    record.state.encode(record.page_count)?;
    let bytes = serde_json::to_vec(record)?;
    let mut staged = tempfile::NamedTempFile::new_in(dir)?;
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    Ok(staged)
}

/// Copy reviewed records. Each completed write is atomic and can safely be retried.
pub fn import(root: &Path, documents: &[BackupDocument]) -> Result<ImportReport, ReadingError> {
    let dir = root.join("documents");
    fs::create_dir_all(&dir)?;
    let mut report = ImportReport::default();
    for record in documents {
        let _lock = lock_document(&dir, &record.key)?;
        let file = document_path(root, &record.key);
        match stage(&dir, record)?.persist_noclobber(&file) {
            Ok(_) => report.added += 1,
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
                let existing: BackupDocument = serde_json::from_slice(&bounded_read(&file, MAX_BACKUP_SIZE)?)?;
                if existing.key == record.key && existing.state == record.state {
                    report.already_present += 1;
                } else {
                    report.conflicts.push(record.key.clone());
                }
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    Ok(report)
}

fn document_path(root: &Path, key: &DocumentKey) -> PathBuf {
    root.join("documents").join(format!("{}.json", key.as_str()))
}

fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>, ReadingError> {
    let mut bytes = Vec::new();
    File::open(path)?.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ReadingError::TooLarge(limit));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_profile_can_be_copied_and_retried_without_overwriting_notes() {
        let legacy = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(legacy.path().join("documents")).unwrap();
        let key = DocumentKey::of(b"source PDF");
        let state = json!({"version": 1, "page": 1, "zoom": "width", "rotation": 0, "annotations": [], "bookmarks": []});
        let source = document_path(legacy.path(), &key);
        fs::write(&source, serde_json::to_vec(&state).unwrap()).unwrap();
        let preview = preview(legacy.path()).unwrap();
        assert_eq!(preview.documents.len(), 1);
        assert_eq!(import(root.path(), &preview.documents).unwrap().added, 1);
        assert_eq!(import(root.path(), &preview.documents).unwrap().already_present, 1);
        let mut changed = preview.documents;
        changed.first_mut().unwrap().state.rotation = 90;
        assert_eq!(import(root.path(), &changed).unwrap().conflicts.as_slice(), std::slice::from_ref(&key));
        assert_eq!(load(root.path(), &key, 1).unwrap().unwrap().rotation, 0);
        assert_eq!(fs::read(source).unwrap(), serde_json::to_vec(&state).unwrap());
    }

    #[test]
    fn stale_edit_cannot_overwrite_a_newer_autosave() {
        let root = tempfile::tempdir().unwrap();
        let key = DocumentKey::of(b"same PDF");
        let mut record = BackupDocument { key: key.clone(), name: None, path: None, page_count: Some(3), state: Reading::default() };
        save(root.path(), &record, None).unwrap();
        let starting = record.state.clone();
        record.state.page = 2;
        save(root.path(), &record, Some(&starting)).unwrap();
        record.state.page = 3;
        assert!(matches!(save(root.path(), &record, Some(&starting)), Err(ReadingError::Conflict)));
        assert_eq!(load(root.path(), &key, 3).unwrap().unwrap().page, 2);
        let previous = checkpoints(root.path(), &key, 3).unwrap();
        assert_eq!(previous.len(), 1);
        assert_eq!(previous[0].state, starting);
    }
}
