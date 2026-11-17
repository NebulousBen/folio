use std::path::Path;

use pdfcraft_engine::folio::library::{self, legacy_ai, session, workspaces};

use crate::{CliError, stdout_line};

/// The legacy profile is opened read-only. Reimporting adds missing records and retains edits.
pub(crate) fn migrate(args: &[String]) -> Result<(), CliError> {
    let (profile, apply) = match args {
        [profile] => (profile, false),
        [profile, flag] if flag == "--apply" => (profile, true),
        _ => return Err("usage: pdfcraft-cli folio-migrate <legacy-profile> [--apply]".into()),
    };
    let legacy = std::fs::canonicalize(Path::new(profile)).map_err(|e| e.to_string())?;
    let native = legacy.join("native");
    if native.exists() && std::fs::canonicalize(&native).map_err(|e| e.to_string())? != native {
        return Err("the native profile must be inside the Folio profile".into());
    }
    if !legacy.join("documents").is_dir() {
        return Err("no Folio documents folder was found in the legacy profile".into());
    }
    let preview = library::preview(&legacy).map_err(|e| e.to_string())?;
    let old_workspaces = workspaces::read(&legacy).map_err(|e| e.to_string())?;
    let tabs = session::read(&legacy).map_err(|e| e.to_string())?;
    let runs = legacy_ai::list(&legacy, None).map_err(|e| e.to_string())?;
    stdout_line(format_args!(
        "{} readings, {} Markdown records excluded, {} unreadable records, {} workspaces, {} saved tabs ({} available), {} previous AI readings ({} verified handoffs).",
        preview.documents.len(),
        preview.excluded_markdown,
        preview.unreadable.len(),
        old_workspaces.len(),
        tabs.tabs.len(),
        tabs.tabs.iter().filter(|tab| tab.path.is_file()).count(),
        runs.len(),
        runs.iter().filter(|run| run.handoff.is_some()).count(),
    ))?;
    if !apply {
        return Ok(());
    }
    if !preview.unreadable.is_empty() {
        return Err("some legacy records could not be read; use Folio’s import preview to review them".into());
    }

    let before = workspaces::read(&native).map_err(|e| e.to_string())?;
    let mut after = before.clone();
    for workspace in old_workspaces {
        if after.iter().all(|existing| existing.id != workspace.id) {
            after.push(workspace);
        }
    }
    let report = library::import(&native, &preview.documents).map_err(|e| e.to_string())?;
    if before != after {
        workspaces::save(&native, &before, &after).map_err(|e| e.to_string())?;
    }
    stdout_line(format_args!(
        "Added {} readings and {} workspaces. Kept {} previously migrated readings and {} newer native readings.",
        report.added,
        after.len().saturating_sub(before.len()),
        report.already_present,
        report.conflicts.len(),
    ))
}
