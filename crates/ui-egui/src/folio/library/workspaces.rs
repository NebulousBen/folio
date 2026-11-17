use std::path::PathBuf;

use pdfcraft_engine::folio::ReadingError;
use pdfcraft_engine::folio::library::workspaces::{self, Listing, Workspace, WorkspaceKind};

use super::{Ask, Job, PdfCraftApp, finished, run};

#[derive(Default)]
pub(super) struct WorkspaceUi {
    open: bool,
    selected: Option<String>,
    name: String,
    search: String,
    listing: Option<Listing>,
    scan: Option<Job<(String, Listing)>>,
}

impl PdfCraftApp {
    pub(crate) fn folio_open_workspaces(&mut self) {
        self.folio.workspaces.open = true;
    }
}

pub(super) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.folio.workspaces.open {
        return;
    }
    let Some(root) = app.folio.directory.clone() else { return };
    let mut state = std::mem::take(&mut app.folio.workspaces);
    if let Some(result) = finished(&mut state.scan) {
        match result {
            Ok((id, listing)) if state.selected.as_ref() == Some(&id) => state.listing = Some(listing),
            Ok(_) => {}
            Err(error) => app.notify_error(error),
        }
    }
    let mut open = state.open;
    egui::Window::new("Reading workspaces").default_width(540.0).open(&mut open).show(ctx, |ui| match workspaces::read(&root) {
        Ok(items) => content(app, ui, &mut state, &items),
        Err(error) => {
            ui.label(error.to_string());
        }
    });
    state.open = open;
    app.folio.workspaces = state;
}

fn content(app: &mut PdfCraftApp, ui: &mut egui::Ui, state: &mut WorkspaceUi, items: &[Workspace]) {
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.name).hint_text("Workspace name")).accessible_name("Workspace name");
        let named = !state.name.trim().is_empty();
        if ui.add_enabled(named, egui::Button::new("New virtual workspace")).clicked() {
            create(app, state.name.trim().to_owned(), WorkspaceKind::Virtual { files: Vec::new() });
            state.name.clear();
        }
        if ui.add_enabled(named, egui::Button::new("Choose workspace folder…")).clicked() {
            let name = state.name.trim().to_owned();
            let dialog = rfd::AsyncFileDialog::new().set_title("Choose a workspace folder");
            app.ask_one(Ask::Folder(dialog), None, move |app, folder| create(app, name, WorkspaceKind::Folder { folder }));
            state.name.clear();
        }
        if ui.button("Import existing workspaces").clicked() {
            change(app, |root, updated| {
                let profile = root.parent().ok_or(ReadingError::Invalid("missing legacy profile"))?;
                let legacy = workspaces::read(profile)?;
                for workspace in legacy {
                    if updated.iter().all(|row| row.id != workspace.id) {
                        updated.push(workspace);
                    }
                }
                Ok(())
            });
        }
    });
    let workspace = items.iter().find(|item| state.selected.as_ref() == Some(&item.id));
    egui::ComboBox::from_label("Workspace").selected_text(workspace.map_or("Choose a workspace", |item| &item.name)).show_ui(ui, |ui| {
        for item in items {
            if ui.selectable_value(&mut state.selected, Some(item.id.clone()), &item.name).clicked() {
                scan(app, ui.ctx(), state, item);
            }
        }
    });
    let Some(workspace) = workspace else { return };
    if state.selected.as_ref() != Some(&workspace.id) {
        return;
    }
    let virtual_files = matches!(workspace.kind, WorkspaceKind::Virtual { .. });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Refresh files").clicked() {
            scan(app, ui.ctx(), state, workspace);
        }
        if virtual_files && ui.button("Add files…").clicked() {
            pick_files(app, workspace.id.clone());
        }
        if ui.add_enabled(!state.name.trim().is_empty(), egui::Button::new("Rename workspace")).clicked() {
            let name = state.name.trim().to_owned();
            change(app, |_, rows| {
                find(rows, &workspace.id)?.name = name;
                Ok(())
            });
            state.name.clear();
        }
        if ui.button("Remove workspace reference").clicked() {
            change(app, |_, rows| {
                rows.retain(|item| item.id != workspace.id);
                Ok(())
            });
            state.selected = None;
            state.listing = None;
        }
    });
    if let WorkspaceKind::Folder { folder } = &workspace.kind {
        ui.label(folder.display().to_string());
    }
    if virtual_files {
        let listing = workspaces::list(workspace);
        files(app, ui, state, workspace, items, &listing);
    } else {
        match state.listing.take() {
            Some(listing) => {
                files(app, ui, state, workspace, items, &listing);
                state.listing = Some(listing);
            }
            None if state.scan.is_some() => {
                ui.spinner();
            }
            _ => {}
        }
    }
}

fn files(app: &mut PdfCraftApp, ui: &mut egui::Ui, state: &mut WorkspaceUi, workspace: &Workspace, items: &[Workspace], listing: &Listing) {
    if listing.capped {
        ui.label("The scan reached its limit. Choose a smaller folder to see the remaining documents.");
    }
    for folder in listing.unavailable.iter().take(10) {
        ui.label(format!("Could not read {}", folder.display()));
    }
    ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Filter files")).accessible_name("Filter files");
    let query = state.search.trim().to_lowercase();
    let matches: Vec<_> = listing.files.iter().filter(|path| query.is_empty() || path.to_string_lossy().to_lowercase().contains(&query)).collect();
    egui::ScrollArea::vertical().max_height(360.0).show_rows(ui, 26.0, matches.len(), |ui, range| {
        for path in &matches[range] {
            ui.push_id(path, |ui| {
                ui.horizontal(|ui| {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if ui.link(name).on_hover_text(path.display().to_string()).clicked() {
                        app.open_path(&path.to_string_lossy());
                    }
                    if matches!(workspace.kind, WorkspaceKind::Virtual { .. }) {
                        ui.menu_button("File actions", |ui| {
                            if ui.button("Remove reference").clicked() {
                                move_reference(app, &workspace.id, None, path);
                                ui.close();
                            }
                            for target in items.iter().filter(|item| item.id != workspace.id && matches!(item.kind, WorkspaceKind::Virtual { .. })) {
                                if ui.button(format!("Move to {}", target.name)).clicked() {
                                    move_reference(app, &workspace.id, Some(&target.id), path);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
            });
        }
    });
}

fn scan(app: &mut PdfCraftApp, ctx: &egui::Context, state: &mut WorkspaceUi, item: &Workspace) {
    state.listing = None;
    state.scan = None;
    if matches!(item.kind, WorkspaceKind::Folder { .. }) {
        let item = item.clone();
        match run("folio-workspace-scan", ctx, move || Ok((item.id.clone(), workspaces::list(&item)))) {
            Ok(job) => state.scan = Some(job),
            Err(error) => app.notify_error(error),
        }
    }
}

fn create(app: &mut PdfCraftApp, name: String, kind: WorkspaceKind) {
    match super::edit::id() {
        Ok(id) => change(app, |_, rows| {
            rows.push(Workspace { id, name, kind });
            Ok(())
        }),
        Err(error) => app.notify_error(error),
    }
}

fn pick_files(app: &mut PdfCraftApp, id: String) {
    let dialog = rfd::AsyncFileDialog::new().set_title("Add files to workspace").add_filter("Documents", &["pdf", "doc", "docx"]);
    app.ask(Ask::Files(dialog), None, move |app, paths| {
        change(app, |_, rows| {
            let files = files_mut(find(rows, &id)?)?;
            files.extend(paths);
            workspaces::deduplicate(files);
            Ok(())
        });
    });
}

fn move_reference(app: &mut PdfCraftApp, source: &str, target: Option<&str>, path: &PathBuf) {
    change(app, |_, rows| {
        files_mut(find(rows, source)?)?.retain(|file| file != path);
        if let Some(target) = target {
            let files = files_mut(find(rows, target)?)?;
            files.push(path.clone());
            workspaces::deduplicate(files);
        }
        Ok(())
    });
}

fn change(app: &mut PdfCraftApp, edit: impl FnOnce(&std::path::Path, &mut Vec<Workspace>) -> Result<(), ReadingError>) {
    let Some(root) = app.folio.directory.as_deref() else { return };
    let result = workspaces::read(root).and_then(|before| {
        let mut after = before.clone();
        edit(root, &mut after)?;
        workspaces::save(root, &before, &after)
    });
    if let Err(error) = result {
        app.notify_error(error);
    }
}

fn find<'a>(rows: &'a mut [Workspace], id: &str) -> Result<&'a mut Workspace, ReadingError> {
    rows.iter_mut().find(|row| row.id == id).ok_or(ReadingError::Invalid("workspace no longer exists"))
}

fn files_mut(item: &mut Workspace) -> Result<&mut Vec<PathBuf>, ReadingError> {
    match &mut item.kind {
        WorkspaceKind::Virtual { files } => Ok(files),
        WorkspaceKind::Folder { .. } => Err(ReadingError::Invalid("folder workspace membership follows the folder")),
    }
}
