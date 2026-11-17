use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pdfcraft_engine::folio::handoff::{connected, import_result};
use pdfcraft_engine::folio::library::handoff::{self, Density, EXCERPT, Handoff, INSTRUCTIONS, RESULT};
use pdfcraft_engine::folio::{DocumentKey, Reading, ReadingError};

use super::{Ask, Job, Notes, PdfCraftApp, codex, commit_reading, finished, run};

#[derive(Default)]
pub(super) struct HandoffUi {
    name: String,
    pages: String,
    purpose: String,
    instructions: String,
    density: Density,
    jobs: Option<Vec<(PathBuf, Handoff)>>,
    preparing: Option<Job<PathBuf>>,
    importing: Option<Job<(Reading, Reading)>>,
    pause: Option<Arc<AtomicBool>>,
}

impl Drop for HandoffUi {
    fn drop(&mut self) {
        if let Some(pause) = self.pause.as_ref() {
            pause.store(true, Ordering::Relaxed);
        }
    }
}

pub(super) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui, notes: &mut Notes, page: u32, count: u32) {
    if let Some(result) = finished(&mut notes.handoff.preparing) {
        match result {
            Ok(path) => {
                notes.handoff.jobs = None;
                app.notify(format!("AI handoff saved in {}", path.display()));
            }
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(result) = finished(&mut notes.handoff.importing) {
        notes.handoff.pause = None;
        match result {
            Ok((_, after)) if notes.reading == after => {
                app.notify_tr("This AI reading is already in your library");
            }
            Ok((before, after)) if notes.reading == before => match commit_reading(app, notes, after) {
                Ok(()) => app.notify_tr("AI reading imported as a layer"),
                Err(error) => app.notify_error(error),
            },
            Ok(_) => app.notify_error("The reading changed during import. Import the PDF again to include the new edits."),
            Err(error) => app.notify_error(error),
        }
    }
    ui.collapsing("AI reading handoffs", |ui| {
        if notes.handoff.pages.is_empty() && !ui.memory(|m| m.has_focus(ui.id().with("handoff_pages"))) {
            notes.handoff.pages = page.to_string();
        }
        ui.add(egui::TextEdit::singleline(&mut notes.handoff.name).hint_text("Reading name")).accessible_name("Reading name");
        ui.add(egui::TextEdit::singleline(&mut notes.handoff.pages).id(ui.id().with("handoff_pages")).hint_text("Physical pages, e.g. 1, 3-5"))
            .accessible_name("Physical pages");
        ui.add(egui::TextEdit::singleline(&mut notes.handoff.purpose).hint_text("Reading purpose")).accessible_name("Reading purpose");
        ui.add(egui::TextEdit::multiline(&mut notes.handoff.instructions).hint_text("Additional instructions").desired_rows(2))
            .accessible_name("AI reading instructions");
        egui::ComboBox::from_label("Highlight density").selected_text(notes.handoff.density.limits().0).show_ui(ui, |ui| {
            for density in [Density::Sparse, Density::Balanced, Density::Comprehensive] {
                ui.selectable_value(&mut notes.handoff.density, density, density.limits().0);
            }
        });
        let ready = notes.handoff.preparing.is_none() && !notes.handoff.name.trim().is_empty();
        if ui.add_enabled(ready, egui::Button::new("Prepare handoff")).clicked()
            && let Err(error) = prepare(app, ui.ctx(), notes, count)
        {
            app.notify_error(error);
        }
        if notes.handoff.preparing.is_some() || notes.handoff.importing.is_some() {
            ui.spinner();
        }
        if let Some(pause) = notes.handoff.pause.as_ref()
            && ui.button("Pause connected reading").clicked()
        {
            pause.store(true, Ordering::Relaxed);
        }
        let Some(dir) = app.folio.directory.as_deref() else { return };
        let home = codex_home(dir);
        if notes.handoff.jobs.is_none() {
            match handoff::for_source(dir, &notes.key) {
                Ok(jobs) => notes.handoff.jobs = Some(jobs),
                Err(error) => {
                    ui.label(error.to_string());
                }
            }
        }
        let mut import = None;
        let mut analyse = None;
        if let Some(jobs) = &notes.handoff.jobs {
            for (folder, handoff) in jobs {
                ui.push_id(folder, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&handoff.name);
                        if ui.link("Show files").clicked()
                            && let Err(error) = crate::menus::tab_context::reveal(&folder.join(INSTRUCTIONS))
                        {
                            app.notify_error(error);
                        }
                        let ready = notes.handoff.importing.is_none();
                        if let Some(binary) = codex::binary() {
                            if ui.add_enabled(ready, egui::Button::new("Run or resume in Codex")).clicked() {
                                analyse = Some((folder.clone(), handoff.clone(), binary));
                            }
                        }
                        if ready && folder.join(RESULT).is_file() && ui.button("Import annotated.pdf").clicked() {
                            import = Some((folder.clone(), handoff.clone(), folder.join(RESULT)));
                        }
                        if ui.add_enabled(ready, egui::Button::new("Choose result PDF…")).clicked() {
                            let ctx = ui.ctx().clone();
                            let id = notes.document;
                            let handoff = handoff.clone();
                            let folder = folder.clone();
                            let dialog = rfd::AsyncFileDialog::new()
                                .set_directory(&folder)
                                .set_title("Choose the annotated result")
                                .add_filter("PDF", &["pdf"]);
                            app.ask_one(Ask::File(dialog), None, move |app, path| {
                                let Some(notes) = app.folio.notes.as_mut().filter(|notes| notes.document == id && notes.key == handoff.source_key)
                                else {
                                    return;
                                };
                                if let Err(error) = start_import(notes, &ctx, folder, handoff, path, count) {
                                    app.notify_error(error);
                                }
                            });
                        }
                    });
                });
            }
        }
        if let Some((folder, handoff, result)) = import
            && let Err(error) = start_import(notes, ui.ctx(), folder, handoff, result, count)
        {
            app.notify_error(error);
        }
        if let Some((folder, handoff, binary)) = analyse
            && let Err(error) = start_analysis(notes, ui.ctx(), folder, handoff, binary, home, count)
        {
            app.notify_error(error);
        }
    });
}

fn codex_home(directory: &Path) -> PathBuf {
    let legacy = directory.parent().map(|profile| profile.join("ai").join("codex"));
    legacy.filter(|profile| profile.is_dir()).unwrap_or_else(|| directory.join("ai").join("codex"))
}

fn start_analysis(
    notes: &mut Notes,
    ctx: &egui::Context,
    folder: PathBuf,
    handoff: Handoff,
    binary: PathBuf,
    home: PathBuf,
    count: u32,
) -> std::io::Result<()> {
    if notes.handoff.importing.is_some() {
        return Ok(());
    }
    let before = notes.reading.clone();
    let pause = Arc::new(AtomicBool::new(false));
    let token = Arc::clone(&pause);
    let job = run("folio-connected-analysis", ctx, move || {
        let excerpt = read_pdf(&folder.join(EXCERPT))?;
        let mut client = None;
        let reading = connected::analyse(&folder, &handoff, excerpt, count, before.clone(), &token, |input, schema| {
            let client = match &mut client {
                Some(client) => client,
                slot => slot.insert(codex::Client::start(&binary, &home, &token)?),
            };
            client.analyse(&folder, input, schema, &token)
        })?;
        Ok((before, reading))
    })?;
    notes.handoff.importing = Some(job);
    notes.handoff.pause = Some(pause);
    Ok(())
}

fn prepare(app: &mut PdfCraftApp, ctx: &egui::Context, notes: &mut Notes, count: u32) -> Result<(), String> {
    let dir = app.folio.directory.clone().ok_or("Folio's reading library is unavailable")?;
    let pages = handoff::parse_ranges(&notes.handoff.pages, count).map_err(|error| error.to_string())?;
    let (excerpt, pages) = app.session.folio_handoff_excerpt(notes.document, &pages).map_err(|error| error.to_string())?;
    let handoff = Handoff {
        version: 1,
        source_key: notes.key.clone(),
        excerpt_key: DocumentKey::of(&excerpt),
        source_name: notes.name.clone(),
        name: notes.handoff.name.trim().to_owned(),
        purpose: notes.handoff.purpose.trim().to_owned(),
        instructions: notes.handoff.instructions.clone(),
        density: notes.handoff.density,
        pages,
    };
    notes.handoff.preparing = Some(run("folio-handoff", ctx, move || handoff::prepare(&dir, &handoff, &excerpt)).map_err(|error| error.to_string())?);
    Ok(())
}

fn start_import(notes: &mut Notes, ctx: &egui::Context, folder: PathBuf, handoff: Handoff, result: PathBuf, count: u32) -> std::io::Result<()> {
    if notes.handoff.importing.is_some() {
        return Ok(());
    }
    let before = notes.reading.clone();
    notes.handoff.importing = Some(run("folio-import-ai-layer", ctx, move || {
        let baseline = read_pdf(&folder.join(EXCERPT))?;
        let result = read_pdf(&result)?;
        let reading = import_result(&handoff, baseline, result, count, before.clone()).map_err(|error| ReadingError::Operation(error.to_string()))?;
        Ok((before, reading))
    })?);
    Ok(())
}

fn read_pdf(path: &Path) -> Result<Arc<Vec<u8>>, ReadingError> {
    const LIMIT: usize = 256 * 1024 * 1024;
    if !fs::metadata(path)?.is_file() {
        return Err(ReadingError::Invalid("choose a PDF file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(ReadingError::TooLarge(LIMIT));
    }
    Ok(Arc::new(bytes))
}
