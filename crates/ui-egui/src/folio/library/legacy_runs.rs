use std::path::Path;

use pdfcraft_engine::folio::library::legacy_ai::{self, LegacyRun};

use super::{Job, PdfCraftApp, finished, run, verify_source};

#[derive(Default)]
pub(super) struct LegacyRunsUi {
    loading: Option<Job<Vec<LegacyRun>>>,
    rows: Option<Vec<LegacyRun>>,
    include_archived: bool,
}

impl PdfCraftApp {
    pub(crate) fn folio_previous_ai_readings(&mut self, ctx: &egui::Context) {
        let Some(old) = self.folio.directory.as_deref().and_then(Path::parent).map(Path::to_owned) else { return };
        if self.folio.legacy_runs.loading.is_some() {
            return;
        }
        match run("folio-previous-ai-readings", ctx, move || legacy_ai::list(&old, None)) {
            Ok(job) => self.folio.legacy_runs.loading = Some(job),
            Err(error) => self.notify_error(error),
        }
    }
}

pub(super) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if let Some(result) = finished(&mut app.folio.legacy_runs.loading) {
        match result {
            Ok(rows) => app.folio.legacy_runs.rows = Some(rows),
            Err(error) => app.notify_error(error),
        }
    }
    let Some(rows) = app.folio.legacy_runs.rows.take() else { return };
    let mut open = true;
    egui::Window::new("Previous Folio AI readings").default_width(580.0).open(&mut open).show(ctx, |ui| {
        ui.checkbox(&mut app.folio.legacy_runs.include_archived, "Include archived readings");
        if rows.is_empty() {
            ui.label("No previous AI readings were found");
        }
        egui::ScrollArea::vertical().max_height(450.0).show(ui, |ui| {
            let include_archived = app.folio.legacy_runs.include_archived;
            for (index, reading) in rows.iter().enumerate().filter(|(_, reading)| include_archived || !reading.archived) {
                ui.push_id(index, |ui| {
                    ui.separator();
                    ui.strong(&reading.name);
                    ui.label(&reading.source_name);
                    ui.small(format!("{} · {}", reading.created_at, reading.status.replace('_', " ")));
                    ui.horizontal_wrapped(|ui| {
                        if reading.source.is_file() && ui.button("Open source PDF").clicked() {
                            verify_source(app, ctx, reading.source.clone(), reading.source_key.clone(), 1);
                        }
                        show_links(app, ui, reading);
                    });
                });
            }
        });
    });
    if open {
        app.folio.legacy_runs.rows = Some(rows);
    }
}

pub(super) fn show_links(app: &mut PdfCraftApp, ui: &mut egui::Ui, reading: &LegacyRun) {
    if let Some(folder) = reading.handoff_folder.as_ref().filter(|folder| folder.is_dir())
        && ui.button("Show handoff folder").clicked()
        && let Err(error) = crate::menus::tab_context::reveal(folder)
    {
        app.notify_error(error);
    }
    if let Some(result) = reading.result.as_ref().filter(|path| path.is_file())
        && ui.button("Open saved result").clicked()
    {
        if let Some(key) = &reading.result_key {
            verify_source(app, ui.ctx(), result.clone(), key.clone(), 1);
        } else {
            app.open_path(&result.to_string_lossy());
        }
    }
}
