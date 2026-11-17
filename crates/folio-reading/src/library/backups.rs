//! Review and restore portable reading-library backups.
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::SystemTime;

use super::{MAX_DOCUMENTS, bounded_read, document_path, is_markdown, load_record, save};
use crate::{Backup, BackupDocument, DocumentKey, MAX_BACKUP_SIZE, MAX_READING_SIZE, Reading, ReadingError};

pub struct Review {
    pub rows: Vec<Row>,
    pub unreadable: Vec<String>,
    pub excluded_markdown: usize,
    pub assign_to_open_pdf: bool,
}

pub struct Row {
    pub incoming: BackupDocument,
    pub previous: Option<Reading>,
}

#[derive(Default)]
pub struct RestoreReport {
    pub added: usize,
    pub replaced: usize,
    pub errors: Vec<String>,
}

/// Build a portable v1 backup of one document or the library. Per-record reads are atomic.
pub fn export(root: &Path, only: Option<&DocumentKey>) -> Result<Vec<u8>, ReadingError> {
    let mut keys = BTreeSet::new();
    if let Some(key) = only {
        keys.insert(key.clone());
    } else {
        let entries = match fs::read_dir(root.join("documents")) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        for (index, entry) in entries.into_iter().flatten().enumerate() {
            if index >= MAX_DOCUMENTS * 2 {
                return Err(ReadingError::Invalid("too many library files"));
            }
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file()
                && path.extension().is_some_and(|extension| extension == "json")
                && let Some(stem) = path.file_stem().and_then(|value| value.to_str())
                && let Ok(key) = DocumentKey::try_from(stem.to_owned())
            {
                keys.insert(key);
            }
        }
    }
    let mut remaining = MAX_BACKUP_SIZE;
    let mut documents = Vec::new();
    for key in keys {
        let file = document_path(root, &key);
        let size = fs::metadata(file)?.len().try_into().map_err(|_| ReadingError::TooLarge(MAX_BACKUP_SIZE))?;
        remaining = remaining.checked_sub(size).ok_or(ReadingError::TooLarge(MAX_BACKUP_SIZE))?;
        if let Some(record) = load_record(root, &key)? {
            record.state.validate(record.page_count)?;
            if !is_markdown(&record) {
                documents.push(record);
            }
        }
    }
    encode(documents)
}

pub fn encode(documents: Vec<BackupDocument>) -> Result<Vec<u8>, ReadingError> {
    let backup = Backup {
        format: "folio-reading-backup".into(),
        version: 1,
        created_at: chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339(),
        documents,
    };
    let bytes = serde_json::to_vec(&backup)?;
    if bytes.len() > MAX_BACKUP_SIZE {
        return Err(ReadingError::TooLarge(MAX_BACKUP_SIZE));
    }
    Ok(bytes)
}

pub fn preview(root: &Path, file: &Path, open_pdf: Option<BackupDocument>) -> Result<Review, ReadingError> {
    let bytes = bounded_read(file, MAX_BACKUP_SIZE)?;
    let (documents, assign_to_open_pdf) = match Backup::decode(&bytes) {
        Ok(backup) => (backup.documents, false),
        Err(error) if bytes.len() <= MAX_READING_SIZE => {
            let state = Reading::decode(&bytes, open_pdf.as_ref().and_then(|record| record.page_count)).map_err(|_| error)?;
            let mut record = open_pdf.ok_or(ReadingError::Invalid("open the associated PDF before importing a single-document backup"))?;
            if !state.source_keys.is_empty() && !state.source_keys.contains(&record.key) {
                return Err(ReadingError::Invalid("this backup belongs to a different PDF"));
            }
            let unverified = state.source_keys.is_empty();
            record.state = state;
            (vec![record], unverified)
        }
        Err(error) => return Err(error),
    };
    review(root, super::classify(documents), assign_to_open_pdf)
}

pub fn review(root: &Path, preview: super::ImportPreview, assign_to_open_pdf: bool) -> Result<Review, ReadingError> {
    let mut rows = Vec::new();
    let mut remaining = MAX_BACKUP_SIZE;
    for incoming in preview.documents {
        let previous = load_record(root, &incoming.key)?.map(|record| record.state);
        if let Some(state) = &previous {
            remaining = remaining.checked_sub(state.encode(None)?.len()).ok_or(ReadingError::TooLarge(MAX_BACKUP_SIZE))?;
        }
        rows.push(Row { incoming, previous });
    }
    Ok(Review { rows, unreadable: preview.unreadable, excluded_markdown: preview.excluded_markdown, assign_to_open_pdf })
}

/// Replace only reviewed and selected records under the same per-document lock used by autosave.
pub fn restore(root: &Path, review: Review, selected: &BTreeSet<DocumentKey>) -> RestoreReport {
    let mut report = RestoreReport::default();
    for row in review.rows {
        if !selected.contains(&row.incoming.key) || row.previous.as_ref() == Some(&row.incoming.state) {
            continue;
        }
        match save(root, &row.incoming, row.previous.as_ref()) {
            Ok(()) if row.previous.is_some() => report.replaced += 1,
            Ok(()) => report.added += 1,
            Err(error) => {
                let name = row.incoming.name.as_deref().unwrap_or(row.incoming.key.as_str());
                report.errors.push(format!("{name}: {error}"));
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_preserves_previous_reading_and_protects_against_a_stale_preview() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let key = DocumentKey::of(b"fixture");
        let initial = BackupDocument { key: key.clone(), name: None, path: None, page_count: None, state: Reading::default() };
        save(one.path(), &initial, None).unwrap();
        let backup = one.path().join("backup.json");
        fs::write(&backup, export(one.path(), None).unwrap()).unwrap();
        let mut local = initial.clone();
        local.state.page = 2;
        save(two.path(), &local, None).unwrap();
        let review = preview(two.path(), &backup, None).unwrap();
        local.state.page = 3;
        save(two.path(), &local, review.rows[0].previous.as_ref()).unwrap();
        let selected = BTreeSet::from([key.clone()]);
        assert_eq!(restore(two.path(), review, &selected).errors.len(), 1);
        let review = preview(two.path(), &backup, None).unwrap();
        assert_eq!(restore(two.path(), review, &selected).replaced, 1);
        assert_eq!(super::super::load(two.path(), &key, 3).unwrap(), Some(initial.state));
        assert!(super::super::checkpoints(two.path(), &key, 3).unwrap().iter().any(|snapshot| snapshot.state == local.state));
    }
}
