use std::collections::BTreeSet;
use std::path::PathBuf;

use pdfcraft_engine::folio::library::backups::{self, RestoreReport, Review};
use pdfcraft_engine::folio::{BackupDocument, DocumentKey, Reading, ReadingError};

use super::{Ask, Job, Notes, PdfCraftApp, finished, notes_for, run};

#[derive(Default)]
pub(super) struct BackupUi {
    loading: Option<Job<Review>>,
    review: Option<Selection>,
    restoring: Option<Job<RestoreReport>>,
    report: Option<RestoreReport>,
    exporting: Option<Job<PathBuf>>,
}

struct Selection {
    review: Review,
    selected: BTreeSet<DocumentKey>,
    confirmed_assignment: bool,
}

impl PdfCraftApp {
    pub(crate) fn folio_pick_backup(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.folio.directory.clone() else { return };
        if self.folio.backups.loading.is_some() || self.folio.backups.restoring.is_some() {
            return;
        }
        let context = ctx.clone();
        let dialog = rfd::AsyncFileDialog::new().set_title("Open a Folio backup").add_filter("Folio backup", &["json"]);
        self.ask_one(Ask::File(dialog), None, move |app, path| {
            let assigned = app.active_ids().and_then(|(_, id)| app.session.get(id)).filter(|doc| !doc.dirty).map(|doc| BackupDocument {
                key: doc.folio_key(),
                name: Some(doc.name.clone()),
                path: doc.path.clone(),
                page_count: u32::try_from(doc.info.pages.len()).ok(),
                state: Reading::default(),
            });
            match run("folio-backup-preview", &context, move || backups::preview(&dir, &path, assigned)) {
                Ok(job) => app.folio.backups.loading = Some(job),
                Err(error) => app.notify_error(error),
            }
        });
    }

    pub(crate) fn folio_export_backup(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.folio.directory.clone() else { return };
        save_dialog(self, ctx, "Folio reading backup.json", move || backups::export(&dir, None));
    }
}

pub(super) fn export_reading(app: &mut PdfCraftApp, ctx: &egui::Context, notes: &Notes) {
    let Some(doc) = app.session.get(notes.document) else { return };
    let record = BackupDocument {
        key: notes.key.clone(),
        name: Some(doc.name.clone()),
        path: doc.path.clone(),
        page_count: u32::try_from(doc.info.pages.len()).ok(),
        state: notes.reading.clone(),
    };
    save_dialog(app, ctx, "Folio reading.json", move || backups::encode(vec![record]));
}

fn save_dialog(app: &mut PdfCraftApp, ctx: &egui::Context, name: &str, make: impl FnOnce() -> Result<Vec<u8>, ReadingError> + Send + 'static) {
    if app.folio.backups.exporting.is_some() {
        return;
    }
    let dialog = rfd::AsyncFileDialog::new().set_title("Save Folio reading backup").set_file_name(name).add_filter("Folio backup", &["json"]);
    let context = ctx.clone();
    app.ask_one(Ask::Save(dialog), None, move |app, path| {
        if path.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("json")) || app.folio.backups.exporting.is_some() {
            app.notify_tr("Choose a JSON file for the reading backup");
            return;
        }
        match run("folio-backup-export", &context, move || {
            let bytes = make()?;
            crate::editing::write_atomically(&path.to_string_lossy(), &bytes)?;
            Ok(path)
        }) {
            Ok(job) => app.folio.backups.exporting = Some(job),
            Err(error) => app.notify_error(error),
        }
    });
}

pub(super) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if let Some(result) = finished(&mut app.folio.backups.exporting) {
        match result {
            Ok(path) => app.notify(format!("Reading backup saved to {}", path.display())),
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(result) = finished(&mut app.folio.backups.loading) {
        match result {
            Ok(review) => {
                let selected = review.rows.iter().filter(|row| row.previous.is_none()).map(|row| row.incoming.key.clone()).collect();
                app.folio.backups.review = Some(Selection { review, selected, confirmed_assignment: false });
            }
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(result) = finished(&mut app.folio.backups.restoring) {
        match result {
            Ok(report) => {
                for view in &mut app.views {
                    view.folio_overlay = None;
                }
                if let Some(id) = app.folio.notes.as_ref().filter(|notes| notes.editor.is_none()).map(|notes| notes.document)
                    && let Ok(updated) = notes_for(app, id)
                {
                    app.folio.notes = updated;
                }
                app.folio.backups.report = Some(report);
            }
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(selection) = app.folio.backups.review.take() {
        show_review(app, ctx, selection);
    }
    if let Some(report) = app.folio.backups.report.take() {
        let mut open = true;
        let mut done = false;
        egui::Window::new("Reading backup results").open(&mut open).show(ctx, |ui| {
            ui.label(format!("{} added · {} replaced · {} errors", report.added, report.replaced, report.errors.len()));
            egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                for error in &report.errors {
                    ui.label(error);
                }
            });
            done = ui.button("Done").clicked();
        });
        if open && !done {
            app.folio.backups.report = Some(report);
        }
    }
}

fn show_review(app: &mut PdfCraftApp, ctx: &egui::Context, mut selection: Selection) {
    let mut open = true;
    let mut restore = false;
    egui::Window::new("Review Folio backup").default_width(530.0).open(&mut open).show(ctx, |ui| {
        if selection.review.assign_to_open_pdf {
            ui.checkbox(&mut selection.confirmed_assignment, "Assign this older backup to the PDF named below");
        }
        if selection.review.excluded_markdown > 0 {
            ui.label(format!("{} Markdown reading records excluded", selection.review.excluded_markdown));
        }
        egui::ScrollArea::vertical().max_height(350.0).show_rows(ui, 26.0, selection.review.rows.len(), |ui, range| {
            for row in &selection.review.rows[range] {
                let key = &row.incoming.key;
                let name = row.incoming.name.as_deref().unwrap_or(key.as_str());
                let same = row.previous.as_ref() == Some(&row.incoming.state);
                let status = if same {
                    "Identical"
                } else if row.previous.is_some() {
                    "Replace and keep checkpoint"
                } else {
                    "Add"
                };
                let mut chosen = selection.selected.contains(key);
                let label = format!("{status} · {name}");
                if ui
                    .add_enabled(!same, egui::Checkbox::new(&mut chosen, label))
                    .on_hover_text(format!(
                        "{} marks · {} bookmarks in backup",
                        row.incoming.state.annotations.len(),
                        row.incoming.state.bookmarks.len()
                    ))
                    .changed()
                {
                    if chosen {
                        selection.selected.insert(key.clone());
                    } else {
                        selection.selected.remove(key);
                    }
                }
            }
        });
        for error in &selection.review.unreadable {
            ui.label(error);
        }
        let ready = (!selection.review.assign_to_open_pdf || selection.confirmed_assignment) && !selection.selected.is_empty();
        restore = ui.add_enabled(ready, egui::Button::new("Restore selected readings")).clicked();
    });
    if restore {
        if let Some(dir) = app.folio.directory.clone() {
            match run("folio-backup-restore", ctx, move || Ok(backups::restore(&dir, selection.review, &selection.selected))) {
                Ok(job) => app.folio.backups.restoring = Some(job),
                Err(error) => app.notify_error(error),
            }
        }
    } else if open {
        app.folio.backups.review = Some(selection);
    }
}
