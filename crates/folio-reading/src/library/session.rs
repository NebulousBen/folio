use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{ErrorKind, Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::bounded_read;
use crate::{DocumentKey, ReadingError};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacySession {
    version: u32,
    #[serde(default = "restore_default")]
    pub restore_on_startup: bool,
    pub tabs: Vec<LegacyTab>,
    pub active_key: Option<DocumentKey>,
    pub closed: Vec<LegacyTab>,
}

#[derive(Deserialize)]
pub struct LegacyTab {
    pub key: DocumentKey,
    pub path: PathBuf,
    pub name: String,
}

const fn restore_default() -> bool {
    true
}

pub fn read(profile: &Path) -> Result<LegacySession, ReadingError> {
    let bytes = match bounded_read(&profile.join("session.json"), 2 * 1024 * 1024) {
        Ok(bytes) => bytes,
        Err(ReadingError::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(LegacySession::default()),
        Err(error) => return Err(error),
    };
    let mut session: LegacySession = serde_json::from_slice(&bytes)?;
    if session.version != 1 || session.tabs.len() > 1_000 || session.closed.len() > 1_000 {
        return Err(ReadingError::Invalid("invalid legacy Folio session"));
    }
    retain_documents(&mut session.tabs);
    retain_documents(&mut session.closed);
    session.closed.truncate(10);
    if session.active_key.as_ref().is_some_and(|key| !session.tabs.iter().any(|tab| &tab.key == key)) {
        session.active_key = session.tabs.first().map(|tab| tab.key.clone());
    }
    Ok(session)
}

fn retain_documents(tabs: &mut Vec<LegacyTab>) {
    let mut keys = BTreeSet::new();
    tabs.retain(|tab| {
        tab.path.is_absolute()
            && tab.path.extension().is_some_and(|extension| ["pdf", "doc", "docx"].iter().any(|format| extension.eq_ignore_ascii_case(format)))
            && keys.insert(tab.key.clone())
    });
}

/// Reuse the PDF that the old reader associated with this Word file. The native copy can
/// be opened and saved as a PDF without writing to the source Word file or the old cache.
pub fn cached_word_preview(native: &Path, tab: &LegacyTab) -> Result<Option<PathBuf>, ReadingError> {
    let Some(extension) = tab.path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase) else { return Ok(None) };
    if !matches!(extension.as_str(), "doc" | "docx") {
        return Ok(None);
    }
    let Some(legacy) = native.parent() else { return Ok(None) };
    let Some(name) = tab.path.file_name() else { return Ok(None) };
    let mut hash = Sha256::new();
    hash.update(format!("folio-word-v1:.{extension}\n").as_bytes());
    let mut source = File::open(&tab.path)?;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    let key: String = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    let mut preview = legacy.join("word").join(key);
    preview.set_extension("pdf");
    if !preview.is_file() {
        return Ok(None);
    }
    let bytes = bounded_read(&preview, 512 * 1024 * 1024)?;
    if !bytes.starts_with(b"%PDF-") || DocumentKey::of(&bytes) != tab.key {
        return Ok(None);
    }
    let dir = native.join("word-previews").join(tab.key.as_str());
    fs::create_dir_all(&dir)?;
    let path = dir.join(name).with_extension("pdf");
    if path.is_file() {
        return Ok(Some(path));
    }
    let mut staged = tempfile::NamedTempFile::new_in(dir)?;
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    match staged.persist_noclobber(&path) {
        Ok(_) => Ok(Some(path)),
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists && path.is_file() => Ok(Some(path)),
        Err(error) => Err(error.error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_only_the_matching_word_preview_and_preserves_the_native_copy() -> Result<(), ReadingError> {
        let profile = tempfile::tempdir()?;
        let dir = profile.path();
        let native = dir.join("native");
        let word = dir.join("Word source.docx");
        fs::write(&word, b"word source bytes")?;
        let key = DocumentKey::of(b"folio-word-v1:.docx\nword source bytes");
        let cache = dir.join("word");
        fs::create_dir(&cache)?;
        let old_preview = cache.join(format!("{}.pdf", key.as_str()));
        let bytes = b"%PDF-1.7\nold Word preview";
        fs::write(&old_preview, bytes)?;

        let tab = LegacyTab { key: DocumentKey::of(bytes), path: word.clone(), name: "Word source".into() };
        let copy = cached_word_preview(&native, &tab)?.ok_or(ReadingError::Invalid("matching Word preview was not found"))?;
        assert!(copy.starts_with(&native));
        assert_eq!(fs::read(&copy)?, bytes);
        fs::write(&copy, b"edited native PDF copy")?;
        assert_eq!(cached_word_preview(&native, &tab)?, Some(copy.clone()));
        assert_eq!(fs::read(copy)?, b"edited native PDF copy");
        assert_eq!(fs::read(&old_preview)?, bytes);

        fs::write(word, b"revised Word source")?;
        assert!(cached_word_preview(&native, &tab)?.is_none());
        Ok(())
    }
}
