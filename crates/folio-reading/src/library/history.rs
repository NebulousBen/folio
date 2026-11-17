use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::bounded_read;
use crate::{DocumentKey, MAX_READING_SIZE, Reading, ReadingError};

const VISIBLE_LIMIT: usize = 20;
const MAX_DIRECTORY_ENTRIES: usize = 50_000;
const MAX_HISTORY_BYTES: usize = 80 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub created_at: String,
    pub reason: String,
    pub state: Reading,
}

pub(super) fn remember(root: &Path, key: &DocumentKey, state: Reading) -> Result<(), ReadingError> {
    let state_hash = DocumentKey::of(&state.encode(None)?);
    let dir = directory(root, key);
    fs::create_dir_all(&dir)?;
    let now = chrono::DateTime::<chrono::Utc>::from(SystemTime::now());
    // Canonicalization supplies Windows' extended path prefix in deeply nested profiles.
    let dir = dir.canonicalize()?;
    let path = dir.join(format!("{}-{}.json", now.timestamp_millis(), &state_hash.as_str()[..16]));
    let checkpoint = Checkpoint { created_at: now.to_rfc3339(), reason: "Before editing".into(), state };
    match super::handoff::store_checkpoint(&path, &serde_json::to_vec(&checkpoint)?) {
        Ok(()) => Ok(()),
        Err(ReadingError::Io(error)) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

/// Read the newest checkpoints from the current library and the old profile. A restore is saved
/// through the same compare-and-swap path and first checkpoints the reading it replaces.
pub fn checkpoints(root: &Path, key: &DocumentKey, page_count: u32) -> Result<Vec<Checkpoint>, ReadingError> {
    let mut paths = entries(&directory(root, key))?;
    if let Some(legacy) = root.parent() {
        paths.extend(entries(&directory(legacy, key))?);
    }
    paths.sort_unstable_by(|a, b| b.file_name().cmp(&a.file_name()));
    let mut total = 0;
    let mut hashes = BTreeSet::new();
    let mut checkpoints = Vec::new();
    for path in paths {
        let bytes = bounded_read(&path, MAX_READING_SIZE + 4096)?;
        total += bytes.len();
        if total > MAX_HISTORY_BYTES {
            break;
        }
        let checkpoint: Checkpoint = serde_json::from_slice(&bytes)?;
        let state = checkpoint.state.encode(Some(page_count))?;
        if hashes.insert(DocumentKey::of(&state)) {
            checkpoints.push(checkpoint);
        }
        if checkpoints.len() >= VISIBLE_LIMIT {
            break;
        }
    }
    Ok(checkpoints)
}

fn directory(root: &Path, key: &DocumentKey) -> PathBuf {
    root.join("history").join(key.as_str())
}

fn entries(dir: &Path) -> Result<Vec<PathBuf>, ReadingError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index == MAX_DIRECTORY_ENTRIES {
            return Err(ReadingError::Invalid("too many history checkpoints"));
        }
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            paths.push(path);
        }
    }
    Ok(paths)
}
