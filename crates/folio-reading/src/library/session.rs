use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

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
