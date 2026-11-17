use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::bounded_read;
use crate::{MAX_READING_SIZE, ReadingError};

const WORKSPACES: &str = "workspaces.json";
const MAX_FILES: usize = 10_000;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: WorkspaceKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkspaceKind {
    Folder { folder: PathBuf },
    Virtual { files: Vec<PathBuf> },
}

pub fn read(root: &Path) -> Result<Vec<Workspace>, ReadingError> {
    let bytes = match bounded_read(&root.join(WORKSPACES), MAX_READING_SIZE) {
        Ok(bytes) => bytes,
        Err(ReadingError::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut items: Vec<Workspace> = serde_json::from_slice(&bytes)?;
    for entry in &mut items {
        if let WorkspaceKind::Virtual { files } = &mut entry.kind {
            // The legacy workspace can contain Markdown sources. Keep the original library file intact.
            files.retain(|file| file.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("md") && !ext.eq_ignore_ascii_case("markdown")));
            deduplicate(files);
        }
    }
    validate(&items)?;
    Ok(items)
}

pub fn save(root: &Path, current: &[Workspace], updated: &[Workspace]) -> Result<(), ReadingError> {
    validate(updated)?;
    fs::create_dir_all(root)?;
    let lock = File::options().create(true).truncate(false).write(true).open(root.join("workspaces.lock"))?;
    lock.lock()?;
    if read(root)? != current {
        return Err(ReadingError::Conflict);
    }
    let bytes = serde_json::to_vec(updated)?;
    if bytes.len() > MAX_READING_SIZE {
        return Err(ReadingError::TooLarge(MAX_READING_SIZE));
    }
    let mut temp = tempfile::NamedTempFile::new_in(root)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(root.join(WORKSPACES)).map_err(|error| error.error)?;
    Ok(())
}

pub fn deduplicate(files: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    files.retain(
        |path| {
            if cfg!(windows) { seen.insert(path.to_string_lossy().to_lowercase()) } else { seen.insert(path.to_string_lossy().into_owned()) }
        },
    );
}

fn readable(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ["pdf", "doc", "docx"].iter().any(|format| ext.eq_ignore_ascii_case(format)))
}

fn validate(items: &[Workspace]) -> Result<(), ReadingError> {
    if items.len() > 500 {
        return Err(ReadingError::Invalid("too many workspaces"));
    }
    let mut ids = HashSet::new();
    for item in items {
        if item.id.is_empty() || !ids.insert(&item.id) || item.name.trim().is_empty() || item.name.chars().count() > 200 {
            return Err(ReadingError::Invalid("workspace name or identifier"));
        }
        let valid = match &item.kind {
            WorkspaceKind::Folder { folder } => folder.is_absolute(),
            WorkspaceKind::Virtual { files } => files.len() <= MAX_FILES && files.iter().all(|file| file.is_absolute() && readable(file)),
        };
        if !valid {
            return Err(ReadingError::Invalid("workspace file references"));
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct Listing {
    pub files: Vec<PathBuf>,
    pub unavailable: Vec<PathBuf>,
    pub capped: bool,
}

pub fn list(item: &Workspace) -> Listing {
    let folder = match &item.kind {
        WorkspaceKind::Folder { folder } => folder,
        WorkspaceKind::Virtual { files } => return Listing { files: files.clone(), ..Listing::default() },
    };
    let mut listing = Listing::default();
    let mut pending = vec![(folder.clone(), 0)];
    let mut visited = 0;
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            listing.unavailable.push(dir);
            continue;
        };
        for entry in entries {
            if visited >= 20_000 || listing.files.len() >= MAX_FILES {
                listing.capped = true;
                break;
            }
            visited += 1;
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name.starts_with("~$") {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if depth < 30 && !is_reparse_point(&entry) {
                    pending.push((path, depth + 1));
                } else if depth >= 30 {
                    listing.capped = true;
                }
            } else if kind.is_file() && readable(&path) {
                listing.files.push(path);
            }
        }
        if visited >= 20_000 || listing.files.len() >= MAX_FILES {
            break;
        }
    }
    listing.files.sort_unstable();
    listing
}

#[cfg(windows)]
fn is_reparse_point(entry: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    entry.metadata().is_ok_and(|meta| meta.file_attributes() & 0x0400 != 0)
}

#[cfg(not(windows))]
fn is_reparse_point(_entry: &fs::DirEntry) -> bool {
    false
}
