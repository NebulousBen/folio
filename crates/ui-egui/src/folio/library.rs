use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use pdfcraft_engine::DocId;
use pdfcraft_engine::folio::{Annotation, AnnotationKind, BackupDocument, DocumentKey, Reading, ReadingError, SelectionSource, library};

use crate::{PdfCraftApp, pickers::Ask};

mod backup_ui;
mod codex;
mod edit;
mod handoff;
mod legacy_runs;
mod proofread;
mod tabs;
mod workspaces;

#[derive(Default)]
pub(crate) struct ReadingUi {
    pub(super) directory: Option<PathBuf>,
    backups: backup_ui::BackupUi,
    tabs: tabs::LegacyTabsUi,
    legacy_runs: legacy_runs::LegacyRunsUi,
    codex_login: Option<codex::Login>,
    importing: Option<Job<library::ImportPreview>>,
    preview: Option<library::ImportPreview>,
    applying: Option<Job<library::ImportReport>>,
    notes: Option<Notes>,
    workspaces: workspaces::WorkspaceUi,
    opening_source: Option<Job<(PathBuf, u32)>>,
}

type Job<T> = Receiver<Result<T, String>>;

struct Notes {
    document: DocId,
    generation: u64,
    name: String,
    key: DocumentKey,
    saved: Option<Reading>,
    reading: Reading,
    filter: String,
    layer: Option<String>,
    brackets_only: bool,
    category: Option<String>,
    selected_bookmarks: BTreeSet<String>,
    editor: Option<edit::Editor>,
    handoff: handoff::HandoffUi,
    history: Option<Result<Vec<library::Checkpoint>, String>>,
}

impl PdfCraftApp {
    pub(crate) fn carry_folio_save(&self, id: DocId, bytes: &std::sync::Arc<Vec<u8>>, destination: &str) -> Result<(), String> {
        let Some(dir) = self.folio.directory.as_deref() else { return Ok(()) };
        let Some(doc) = self.session.get(id) else { return Ok(()) };
        let saved = doc.saved_bytes();
        if saved.as_slice() == bytes.as_slice() {
            return Ok(());
        }
        let old = DocumentKey::of(&saved);
        let Some(reading) = library::load_for_revision(dir, &old).map_err(|error| error.to_string())? else { return Ok(()) };
        let Some((key, state)) = doc.rebase_folio_reading(bytes, &reading).map_err(|error| error.to_string())? else {
            return Err("Folio reading data stays attached to the previous PDF revision".into());
        };
        let retained_all = state.annotations.len() == reading.annotations.len() && state.bookmarks.len() == reading.bookmarks.len();
        let name = Path::new(destination).file_name().and_then(|name| name.to_str()).map(str::to_owned);
        let record = BackupDocument { key, name, path: Some(destination.into()), page_count: Some(doc.info.pages.len() as u32), state };
        let report = library::import(dir, &[record]).map_err(|error| error.to_string())?;
        if !report.conflicts.is_empty() {
            return Err("The saved PDF already has its own Folio reading data".into());
        }
        if !retained_all {
            return Err("Folio marks on changed pages stay attached to the previous PDF revision".into());
        }
        Ok(())
    }

    pub fn set_folio_data_dir(&mut self, directory: Option<PathBuf>) {
        self.folio.directory = directory;
    }

    pub(crate) fn folio_preview_profile(&mut self, ctx: &egui::Context) {
        let Some(path) = self.folio.directory.as_deref().and_then(Path::parent) else { return };
        self.start_folio_import(path.to_path_buf(), ctx);
    }

    fn start_folio_import(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.folio.importing.is_some() || self.folio.applying.is_some() {
            return;
        }
        match run("folio-reading-preview", ctx, move || library::preview(&path)) {
            Ok(job) => {
                self.folio.preview = None;
                self.folio.importing = Some(job);
            }
            Err(error) => self.notify_error(error),
        }
    }

    pub(crate) fn show_folio_notes(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        match notes_for(self, id) {
            Ok(Some(notes)) => self.folio.notes = Some(notes),
            Ok(None) => self.notify_tr("Save this PDF before opening its Folio reading"),
            Err(error) => self.notify_error(error),
        }
    }
}

fn notes_for(app: &PdfCraftApp, id: DocId) -> Result<Option<Notes>, String> {
    let Some(document) = app.session.get(id) else { return Ok(None) };
    if document.dirty {
        return Ok(None);
    }
    document.check_copying().map_err(|error| error.to_string())?;
    let key = document.folio_key();
    let local = if let Some(dir) = app.folio.directory.as_deref() {
        let pages = u32::try_from(document.info.pages.len()).map_err(|error| error.to_string())?;
        library::load(dir, &key, pages).map_err(|error| error.to_string())?
    } else {
        None
    };
    let reading = match &local {
        Some(reading) => reading.clone(),
        None => document.folio_reading().map_err(|error| error.to_string())?.unwrap_or_default(),
    };
    Ok(Some(Notes {
        document: id,
        generation: document.edit_generation(),
        name: document.name.clone(),
        key,
        saved: local,
        reading,
        filter: String::new(),
        layer: None,
        brackets_only: false,
        category: None,
        selected_bookmarks: BTreeSet::new(),
        editor: None,
        handoff: Default::default(),
        history: None,
    }))
}

fn commit_reading(app: &mut PdfCraftApp, notes: &mut Notes, state: Reading) -> Result<(), String> {
    let dir = app.folio.directory.as_deref().ok_or("Folio's reading library is unavailable")?;
    let doc = app.session.get(notes.document).ok_or("This document was closed")?;
    if doc.dirty || doc.edit_generation() != notes.generation || doc.folio_key() != notes.key {
        return Err("The PDF changed while the reading was open".into());
    }
    let fields = BackupDocument {
        key: notes.key.clone(),
        name: Some(doc.name.clone()),
        path: doc.path.clone(),
        page_count: Some(u32::try_from(doc.info.pages.len()).map_err(|error| error.to_string())?),
        state,
    };
    library::save(dir, &fields, notes.saved.as_ref()).map_err(|error| error.to_string())?;
    notes.reading = fields.state;
    notes.saved = Some(notes.reading.clone());
    notes.history = None;
    if let Some(view) = app.views.iter_mut().find(|view| view.id == notes.document) {
        view.folio_overlay = None;
    }
    Ok(())
}

pub(crate) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    backup_ui::show(app, ctx);
    tabs::show(app, ctx);
    legacy_runs::show(app, ctx);
    codex::poll_login(app);
    workspaces::show(app, ctx);
    if let Some(result) = finished(&mut app.folio.opening_source) {
        match result {
            Ok((path, page)) => app.open_folio_source(&path.to_string_lossy(), page),
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(result) = finished(&mut app.folio.importing) {
        match result {
            Ok(preview) => app.folio.preview = Some(preview),
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(result) = finished(&mut app.folio.applying) {
        match result {
            Ok(report) => {
                for view in &mut app.views {
                    view.folio_overlay = None;
                }
                app.notify(format!(
                    "Imported {} readings. {} already present. {} conflicts.",
                    report.added,
                    report.already_present,
                    report.conflicts.len()
                ));
            }
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(preview) = app.folio.preview.take() {
        show_preview(app, ctx, preview);
    }
    if let Some(notes) = app.folio.notes.take() {
        show_notes(app, ctx, notes);
    }
}

fn show_preview(app: &mut PdfCraftApp, ctx: &egui::Context, preview: library::ImportPreview) {
    let mut open = true;
    let mut apply = false;
    egui::Window::new("Import Folio reading data").default_width(450.0).open(&mut open).show(ctx, |ui| {
        ui.label(format!("{} readings ready to import", preview.documents.len()));
        if preview.excluded_markdown > 0 {
            ui.label(format!("{} Markdown readings in the original library", preview.excluded_markdown));
        }
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for entry in preview.documents.iter().take(100) {
                let name = entry.name.as_deref().or(entry.path.as_deref()).unwrap_or(entry.key.as_str());
                ui.label(format!("{name} · {} annotations · {} bookmarks", entry.state.annotations.len(), entry.state.bookmarks.len()));
            }
            for error in preview.unreadable.iter().take(10) {
                ui.label(error);
            }
        });
        if preview.documents.len() > 100 || preview.unreadable.len() > 10 {
            ui.label("Showing up to 100 readings and 10 errors.");
        }
        apply = ui.add_enabled(!preview.documents.is_empty(), egui::Button::new("Import readings")).clicked();
    });
    if apply {
        if let Some(dir) = app.folio.directory.clone() {
            match run("folio-reading-import", ctx, move || library::import(&dir, &preview.documents)) {
                Ok(job) => app.folio.applying = Some(job),
                Err(error) => app.notify_error(error),
            }
        }
    } else if open {
        app.folio.preview = Some(preview);
    }
}

fn show_notes(app: &mut PdfCraftApp, ctx: &egui::Context, mut notes: Notes) {
    if app.session.get(notes.document).is_none_or(|doc| doc.dirty || doc.edit_generation() != notes.generation) {
        return;
    }
    let mut open = true;
    let mut jump = None;
    let mut export = false;
    let mut pdf_report = false;
    let mut extract = false;
    let mut annotated = false;
    let mut restore_answers = false;
    let target = app.active_ids().filter(|&(_, id)| id == notes.document);
    let page = target.and_then(|(index, _)| app.views.get(index)).map(|view| view.current as u32 + 1);
    let page_count = app.session.get(notes.document).map_or(1, |doc| doc.info.pages.len()) as u32;
    egui::Window::new(format!("Reading notes · {}", notes.name))
        .id(egui::Id::unique("folio_reading_notes"))
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                export = ui.button("Save notes as Markdown…").clicked();
                pdf_report = ui.button("Save notes as PDF…").clicked();
                annotated = ui.button("Save annotated copy…").clicked();
                if ui.button("Back up this reading…").clicked() {
                    backup_ui::export_reading(app, ui.ctx(), &notes);
                }
            });
            if let Some(page) = page {
                edit::tools(app, ui, &mut notes, page, page_count);
                source_links(app, ui, &notes, page);
                handoff::show(app, ui, &mut notes, page, page_count);
                show_history(app, ui, &mut notes, page_count);
                if !notes.reading.form_values.is_empty() {
                    restore_answers = ui.button("Restore Folio form answers").clicked();
                }
            }
            jump = notes_ui(ui, &mut notes);
            if !notes.reading.bookmarks.is_empty() {
                extract = ui.add_enabled(!notes.selected_bookmarks.is_empty(), egui::Button::new("Save selected bookmark pages…")).clicked();
            }
        });
    if export {
        export_notes(app, &notes);
    }
    if pdf_report {
        export_pdf_report(app, &notes);
    }
    if extract {
        export_bookmarks(app, &notes);
    }
    if annotated {
        let reading = notes.reading.clone();
        let id = notes.document;
        let dialog = rfd::AsyncFileDialog::new().set_file_name("Annotated reading.pdf").add_filter("PDF", &["pdf"]);
        app.ask_one(Ask::Save(dialog), None, move |app, path| match app.session.folio_annotated_copy(id, &reading) {
            Ok(bytes) => save_new_pdf(app, &path, &bytes),
            Err(error) => app.notify_error(error),
        });
    }
    if restore_answers {
        apply_form_answers(app, &notes);
    }
    if let Some(page) = jump
        && let Some(index) = app.views.iter().position(|view| view.id == notes.document)
    {
        app.active = Some(index);
        if let Some(view) = app.views.get_mut(index) {
            view.go_to_page(page.saturating_sub(1) as usize);
        }
    }
    if open && app.session.get(notes.document).is_some() {
        app.folio.notes = Some(notes);
    }
}

fn apply_form_answers(app: &mut PdfCraftApp, notes: &Notes) {
    if !app.active_ids().is_some_and(|(_, id)| id == notes.document) {
        return;
    }
    let Some(doc) = app.session.get(notes.document) else { return };
    let restore = doc.folio_form_restore(&notes.reading);
    if restore.edits.is_empty() {
        app.notify_tr("No compatible form answers were found in this PDF");
        return;
    }
    let edit = pdfcraft_engine::Edit::Batch { label: "Restore Folio form answers".into(), edits: restore.edits };
    if app.apply_edit(edit) {
        app.notify(format!("Restored Folio form answers. {} entries skipped. Save the PDF to keep the restored answers.", restore.skipped));
    }
}

fn show_history(app: &mut PdfCraftApp, ui: &mut egui::Ui, notes: &mut Notes, page_count: u32) {
    let Some(dir) = app.folio.directory.as_deref() else { return };
    let mut restore = None;
    ui.collapsing("Reading history", |ui| {
        let history = notes.history.get_or_insert_with(|| library::checkpoints(dir, &notes.key, page_count).map_err(|error| error.to_string()));
        match history {
            Ok(checkpoints) if checkpoints.is_empty() => {
                ui.label("No checkpoints are available yet");
            }
            Ok(checkpoints) => {
                egui::ScrollArea::vertical().id_salt("reading_checkpoints").max_height(180.0).show(ui, |ui| {
                    for (index, entry) in checkpoints.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!("{} · {}", entry.created_at, entry.reason));
                                ui.small(format!("{} marks · {} bookmarks", entry.state.annotations.len(), entry.state.bookmarks.len()));
                                if ui.button("Restore checkpoint").clicked() {
                                    restore = Some(entry.state.clone());
                                }
                            });
                        });
                    }
                });
            }
            Err(error) => {
                ui.label(error.as_str());
            }
        }
    });
    if let Some(state) = restore {
        match commit_reading(app, notes, state) {
            Ok(()) => {
                notes.editor = None;
                notes.layer = None;
                notes.category = None;
                notes.selected_bookmarks.clear();
                app.notify_tr("Reading checkpoint restored");
            }
            Err(error) => app.notify_error(error),
        }
    }
}

fn source_links(app: &mut PdfCraftApp, ui: &mut egui::Ui, notes: &Notes, page: u32) {
    for source in &notes.reading.selection_sources {
        let Some(mapping) = source.pages.iter().find(|item| item.page == page) else { continue };
        ui.horizontal_wrapped(|ui| {
            if ui.link(format!("{} · source page {}", source.name, mapping.source_page)).clicked() {
                return_to_source(app, ui.ctx(), source, mapping.source_page);
            }
            if ui.small_button("Locate source PDF…").clicked() {
                pick_source(app, ui.ctx(), source.key.clone(), mapping.source_page);
            }
        });
    }
}

fn return_to_source(app: &mut PdfCraftApp, ctx: &egui::Context, source: &SelectionSource, page: u32) {
    if let Some(index) = app.views.iter().position(|view| app.session.get(view.id).is_some_and(|doc| !doc.dirty && doc.folio_key() == source.key)) {
        app.active = Some(index);
        app.views[index].go_to_page(page.saturating_sub(1) as usize);
        return;
    }
    let local = app.folio.directory.as_deref().and_then(|dir| library::local_pdf(dir, &source.key).ok().flatten());
    if let Some(path) = local.filter(|path| path.is_file()) {
        verify_source(app, ctx, path, source.key.clone(), page);
    } else {
        pick_source(app, ctx, source.key.clone(), page);
    }
}

fn pick_source(app: &mut PdfCraftApp, ctx: &egui::Context, key: DocumentKey, page: u32) {
    let ctx = ctx.clone();
    let dialog = rfd::AsyncFileDialog::new().set_title("Locate the original PDF").add_filter("PDF", &["pdf"]);
    app.ask_one(Ask::File(dialog), None, move |app, path| verify_source(app, &ctx, path, key, page));
}

fn verify_source(app: &mut PdfCraftApp, ctx: &egui::Context, path: PathBuf, key: DocumentKey, page: u32) {
    match run("folio-source-verify", ctx, move || {
        library::verify_pdf(&path, &key)?;
        Ok((path, page))
    }) {
        Ok(job) => app.folio.opening_source = Some(job),
        Err(error) => app.notify_error(error),
    }
}

fn notes_ui(ui: &mut egui::Ui, notes: &mut Notes) -> Option<u32> {
    ui.add(egui::TextEdit::singleline(&mut notes.filter).hint_text("Search notes")).accessible_name("Search notes");
    ui.checkbox(&mut notes.brackets_only, "Brackets only");
    egui::ComboBox::from_label("Reading layer")
        .selected_text(
            notes.layer.as_deref().and_then(|id| notes.reading.layers.iter().find(|layer| layer.id == id)).map_or("All layers", |layer| &layer.title),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut notes.layer, None, "All layers");
            for layer in &notes.reading.layers {
                ui.selectable_value(&mut notes.layer, Some(layer.id.clone()), &layer.title);
            }
        });
    egui::ComboBox::from_label("Category")
        .selected_text(notes.category.as_deref().map_or("All categories", |id| category_label(&notes.reading, id)))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut notes.category, None, "All categories");
            for category in &notes.reading.highlight_colors {
                ui.selectable_value(&mut notes.category, Some(category.id.clone()), &category.label);
            }
        });
    let mut jump = None;
    egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
        if !notes.reading.bookmarks.is_empty() {
            ui.collapsing("Personal bookmarks", |ui| {
                for bookmark in &notes.reading.bookmarks {
                    ui.horizontal_wrapped(|ui| {
                        let mut selected = notes.selected_bookmarks.contains(&bookmark.id);
                        if ui.checkbox(&mut selected, "").accessible_name(format!("Select {}", bookmark.label)).changed() {
                            if selected {
                                notes.selected_bookmarks.insert(bookmark.id.clone());
                            } else {
                                notes.selected_bookmarks.remove(&bookmark.id);
                            }
                        }
                        if ui.link(format!("{} · page {}", bookmark.label, bookmark.page)).clicked() {
                            jump = Some(bookmark.page);
                        }
                        if let Some(end) = bookmark.end_page
                            && ui.link(format!("End on page {end}")).clicked()
                        {
                            jump = Some(end);
                        }
                        if ui.small_button("Edit").clicked() {
                            notes.editor = Some(edit::Editor::Bookmark(bookmark.clone()));
                        }
                    });
                }
            });
        }
        let mut editor = None;
        for entry in filtered_notes(notes) {
            ui.separator();
            if ui.link(format!("Page {}", entry.page)).clicked() {
                jump = Some(entry.page);
            }
            if let Some(title) = &entry.title {
                ui.strong(title);
            }
            ui.label(&entry.quote);
            if !entry.comment.is_empty() {
                ui.label(&entry.comment);
                ui.small(super::word_count(&entry.comment));
            }
            if ui.small_button("Edit reading mark").clicked() {
                editor = Some(edit::Editor::annotation(entry.clone()));
            }
        }
        if let Some(editor) = editor {
            notes.editor = Some(editor);
        }
    });
    jump
}

fn export_bookmarks(app: &mut PdfCraftApp, notes: &Notes) {
    let labels: Vec<_> = notes.selected_bookmarks.iter().map(String::as_str).collect();
    let bytes = match app.session.extract_folio_bookmarks(notes.document, &notes.reading, &labels) {
        Ok(bytes) => bytes,
        Err(error) => {
            app.notify_error(error);
            return;
        }
    };
    let dialog = rfd::AsyncFileDialog::new().set_file_name("Reading selection.pdf").add_filter("PDF", &["pdf"]);
    app.ask_one(Ask::Save(dialog), None, move |app, path| save_new_pdf(app, &path, &bytes));
}

fn save_new_pdf(app: &mut PdfCraftApp, path: &Path, bytes: &[u8]) {
    let destination = path.to_string_lossy();
    if app.session.docs().iter().filter_map(|doc| doc.path.as_deref()).any(|source| crate::same_file(source, &destination)) {
        app.notify_tr("Choose a separate file for the export");
        return;
    }
    match crate::editing::write_atomically(&destination, bytes) {
        Ok(()) => app.notify_tr("PDF saved"),
        Err(error) => app.notify_error(error),
    }
}

fn category_label<'a>(reading: &'a Reading, id: &'a str) -> &'a str {
    reading.highlight_colors.iter().find(|category| category.id == id).map_or(id, |category| category.label.as_str())
}

fn filtered_notes(notes: &Notes) -> impl Iterator<Item = &Annotation> {
    let query = notes.filter.trim().to_lowercase();
    notes.reading.annotations.iter().filter(move |mark| {
        (!notes.brackets_only || mark.kind == AnnotationKind::Section)
            && notes.layer.as_ref().is_none_or(|id| mark.layer_id.as_ref() == Some(id))
            && notes.category.as_deref().is_none_or(|id| mark.color == id)
            && (query.is_empty()
                || [mark.title.as_deref().unwrap_or_default(), &mark.quote, &mark.comment, category_label(&notes.reading, &mark.color)]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&query)))
    })
}

fn export_notes(app: &mut PdfCraftApp, notes: &Notes) {
    use std::fmt::Write as _;

    let mut body = format!("# Reading notes — {}\n\n", markdown_text(&notes.name));
    for mark in filtered_notes(notes) {
        let category = category_label(&notes.reading, &mark.color);
        let _ = writeln!(body, "## Page {} · {}\n", mark.page, markdown_text(category));
        if let Some(title) = &mark.title {
            let _ = writeln!(body, "**{}**\n", markdown_text(title));
        }
        for line in mark.quote.lines() {
            let _ = writeln!(body, "> {}", markdown_text(line));
        }
        let _ = writeln!(body, "\n{}\n", markdown_text(&mark.comment));
    }
    let dialog = rfd::AsyncFileDialog::new().set_file_name("Reading notes.md").add_filter("Markdown", &["md"]);
    app.ask_one(Ask::Save(dialog), None, move |app, path| {
        if path.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("md")) {
            app.notify_tr("Choose a .md file for the notes");
            return;
        }
        match crate::editing::write_atomically(&path.to_string_lossy(), body.as_bytes()) {
            Ok(()) => app.notify_tr("Reading notes saved"),
            Err(error) => app.notify_error(error),
        }
    });
}

fn export_pdf_report(app: &mut PdfCraftApp, notes: &Notes) {
    use std::fmt::Write as _;

    let title = format!("Reading notes — {}", notes.name);
    let mut report = format!("{title}\n\n");
    for mark in filtered_notes(notes) {
        let _ = writeln!(report, "Page {} · {}", mark.page, category_label(&notes.reading, &mark.color));
        if let Some(title) = &mark.title {
            let _ = writeln!(report, "{title}");
        }
        if !mark.quote.is_empty() {
            let _ = writeln!(report, "{}\n", mark.quote);
        }
        if !mark.comment.is_empty() {
            let _ = writeln!(report, "{}", mark.comment);
        }
        report.push('\n');
    }
    let pdf = match app.session.folio_notes_report(notes.document, &title, &report) {
        Ok(pdf) => pdf,
        Err(error) => {
            app.notify_error(error);
            return;
        }
    };
    let dialog = rfd::AsyncFileDialog::new().set_file_name("Reading notes.pdf").add_filter("PDF", &["pdf"]);
    app.ask_one(Ask::Save(dialog), None, move |app, path| save_new_pdf(app, &path, &pdf));
}

fn markdown_text(text: &str) -> String {
    let mut escaped = String::new();
    for character in text.chars() {
        if matches!(character, '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '<' | '>' | '&' | '#' | '-' | '+' | '!' | '|' | '~') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn run<T: Send + 'static>(
    name: &str,
    ctx: &egui::Context,
    job: impl FnOnce() -> Result<T, ReadingError> + Send + 'static,
) -> std::io::Result<Job<T>> {
    let ctx = ctx.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new().name(name.into()).spawn(move || {
        let _ = sender.send(job().map_err(|error| error.to_string()));
        ctx.request_repaint();
    })?;
    Ok(receiver)
}

fn finished<T>(slot: &mut Option<Job<T>>) -> Option<Result<T, String>> {
    let result = match slot.as_ref()?.try_recv() {
        Ok(result) => result,
        Err(TryRecvError::Empty) => return None,
        Err(TryRecvError::Disconnected) => Err("The reading library operation stopped unexpectedly".into()),
    };
    *slot = None;
    Some(result)
}
