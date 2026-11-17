//! Access preserved AI readings in the Electron profile. Their files remain at the original paths.
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::bounded_read;
use super::handoff::{Density, EXCERPT, Handoff, Page, RESULT};
use crate::{DocumentKey, ReadingError};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id: String,
    version: u32,
    created_at: String,
    status: String,
    packet_sha256: Option<DocumentKey>,
    packet: Packet,
    source_path: Option<PathBuf>,
    handoff_folder: Option<PathBuf>,
    output_path: Option<PathBuf>,
    output_sha256: Option<DocumentKey>,
    #[serde(default)]
    archived: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Packet {
    name: String,
    source_sha256: DocumentKey,
    settings: Settings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    mode: String,
    reading_name: Option<String>,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    instructions: String,
    #[serde(default)]
    density: Density,
    #[serde(default)]
    pages: Vec<u32>,
}

pub struct LegacyRun {
    pub name: String,
    pub source_name: String,
    pub source_key: DocumentKey,
    pub created_at: String,
    pub status: String,
    pub archived: bool,
    pub source: PathBuf,
    pub handoff_folder: Option<PathBuf>,
    pub result: Option<PathBuf>,
    pub result_key: Option<DocumentKey>,
    pub handoff: Option<Handoff>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    run_id: String,
    packet_sha256: DocumentKey,
    source_sha256: DocumentKey,
    pdf_sha256: DocumentKey,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageMap {
    version: u32,
    pdf_sha256: Option<DocumentKey>,
    pages: Vec<MappedPage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MappedPage {
    pdf_page: u32,
    source_page: u32,
    label: String,
}

pub fn list(profile: &Path, only: Option<&DocumentKey>) -> Result<Vec<LegacyRun>, ReadingError> {
    let dir = profile.join("ai").join("runs");
    let rows = match fs::read_dir(dir) {
        Ok(rows) => rows,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut runs = Vec::new();
    for entry in rows.take(10_000) {
        let entry = entry?;
        let folder = entry.path();
        let name = entry.file_name();
        let Some(id) = name.to_str().filter(|id| valid_id(id)) else { continue };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Ok(bytes) = bounded_read(&folder.join("run.json"), 64 * 1024 * 1024) else { continue };
        let Ok(record) = serde_json::from_slice::<Record>(&bytes) else { continue };
        if record.version != 1 || record.id != id || only.is_some_and(|key| key != &record.packet.source_sha256) || markdown(&record.packet.name) {
            continue;
        }
        runs.push(to_run(folder, record));
    }
    runs.sort_unstable_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(runs)
}

fn to_run(folder: PathBuf, record: Record) -> LegacyRun {
    let settings = &record.packet.settings;
    let name = settings.reading_name.as_deref().filter(|name| !name.trim().is_empty()).unwrap_or(&settings.topic);
    let name = if name.trim().is_empty() { record.packet.name.clone() } else { name.to_owned() };
    let handoff_folder = record.handoff_folder.as_ref().filter(|path| path.is_absolute()).cloned();
    let handoff = handoff_folder.as_deref().filter(|_| settings.mode == "manual").and_then(|path| verified_manifest(path, &record, &name).ok());
    let result = record
        .output_path
        .filter(|path| is_pdf(path) && path.is_file())
        .or_else(|| handoff_folder.as_ref().map(|path| path.join(RESULT)).filter(|path| path.is_file()));
    let source = record.source_path.filter(|path| is_pdf(path) && path.is_file()).unwrap_or_else(|| folder.join("source.pdf"));
    LegacyRun {
        name,
        source_name: record.packet.name,
        source_key: record.packet.source_sha256,
        created_at: record.created_at,
        status: record.status,
        archived: record.archived,
        source,
        handoff_folder,
        result,
        result_key: record.output_sha256,
        handoff,
    }
}

fn verified_manifest(folder: &Path, record: &Record, name: &str) -> Result<Handoff, ReadingError> {
    let manifest: Manifest = serde_json::from_slice(&bounded_read(&folder.join("handoff.json"), 1024 * 1024)?)?;
    let map: PageMap = serde_json::from_slice(&bounded_read(&folder.join("pages.json"), 2 * 1024 * 1024)?)?;
    let settings = &record.packet.settings;
    // The first two handoff formats could reuse a packet's folder under a different run ID.
    let early = matches!(manifest.version, 1 | 2) && map.version == manifest.version;
    if !valid_id(&manifest.run_id)
        || (!early && (manifest.version != 3 || map.version != 3 || manifest.run_id != record.id))
        || record.packet_sha256.as_ref() != Some(&manifest.packet_sha256)
        || manifest.source_sha256 != record.packet.source_sha256
        || map.pdf_sha256.as_ref().map_or(!early, |key| key != &manifest.pdf_sha256)
        || !settings.pages.iter().copied().eq(map.pages.iter().map(|page| page.source_page))
        || map.pages.iter().enumerate().any(|(index, page)| page.pdf_page != index as u32 + 1)
    {
        return Err(ReadingError::Invalid("legacy handoff manifest does not match its reading"));
    }
    super::verify_pdf(&folder.join(EXCERPT), &manifest.pdf_sha256)?;
    let handoff = Handoff {
        version: 1,
        source_key: manifest.source_sha256,
        excerpt_key: manifest.pdf_sha256,
        source_name: record.packet.name.clone(),
        name: name.to_owned(),
        purpose: settings.topic.clone(),
        instructions: settings.instructions.clone(),
        density: settings.density,
        pages: map.pages.into_iter().map(|page| Page { source_page: page.source_page, label: page.label }).collect(),
    };
    handoff.validate()?;
    Ok(handoff)
}

fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().all(|character| character.is_ascii_hexdigit() || character == b'-')
}

fn is_pdf(path: &Path) -> bool {
    path.is_absolute() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn markdown(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}
