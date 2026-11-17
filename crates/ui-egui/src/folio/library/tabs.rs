use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use pdfcraft_engine::folio::library::session::{self, LegacySession};

use super::{Job, PdfCraftApp, finished, run};
use crate::OpenOutcome;

#[derive(Default)]
pub(super) struct LegacyTabsUi {
    loading: Option<Job<LegacySession>>,
    preview: Option<LegacySession>,
    opening: Option<Opening>,
}

struct Opening {
    queue: VecDeque<String>,
    waiting: Option<String>,
    active: Option<String>,
    unavailable: usize,
}

impl PdfCraftApp {
    pub(crate) fn folio_preview_tabs(&mut self, ctx: &egui::Context) {
        let Some(profile) = self.folio.directory.as_deref().and_then(Path::parent).map(Path::to_owned) else { return };
        if self.folio.tabs.loading.is_some() || self.folio.tabs.opening.is_some() {
            return;
        }
        match run("folio-tabs-preview", ctx, move || session::read(&profile)) {
            Ok(job) => self.folio.tabs.loading = Some(job),
            Err(error) => self.notify_error(error),
        }
    }
}

pub(super) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if let Some(result) = finished(&mut app.folio.tabs.loading) {
        match result {
            Ok(session) => app.folio.tabs.preview = Some(session),
            Err(error) => app.notify_error(error),
        }
    }
    if let Some(session) = app.folio.tabs.preview.take() {
        let mut open = true;
        let mut apply = false;
        egui::Window::new("Restore previous Folio tabs").default_width(490.0).open(&mut open).show(ctx, |ui| {
            let usable = session.tabs.iter().filter(|tab| tab.path.is_file()).count();
            ui.label(format!("{usable} saved tabs available"));
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for tab in &session.tabs {
                    let suffix = if !tab.path.is_file() {
                        " · Missing file"
                    } else if session.active_key.as_ref() == Some(&tab.key) {
                        " · Previously active"
                    } else {
                        ""
                    };
                    ui.label(format!("{}{suffix}", tab.name));
                }
            });
            apply = ui.add_enabled(usable > 0 || !session.closed.is_empty(), egui::Button::new("Restore saved tabs")).clicked();
        });
        if apply {
            start(app, session);
        } else if open {
            app.folio.tabs.preview = Some(session);
        }
    }
    if let Some(mut opening) = app.folio.tabs.opening.take() {
        if open_next(app, &mut opening) {
            app.folio.tabs.opening = Some(opening);
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
}

fn start(app: &mut PdfCraftApp, session: LegacySession) {
    let active = session.active_key.as_ref().and_then(|key| session.tabs.iter().find(|tab| &tab.key == key)).map(|tab| text_path(&tab.path));
    for tab in session.closed.iter().rev() {
        let path = text_path(&tab.path);
        if !app.closed_tabs.iter().any(|current| crate::same_file(current, &path)) {
            app.closed_tabs.push(path);
        }
    }
    if app.closed_tabs.len() > 10 {
        let old = app.closed_tabs.len() - 10;
        app.closed_tabs.drain(..old);
    }
    app.reopen_last_session = session.restore_on_startup;
    let queue = session.tabs.iter().map(|tab| text_path(&tab.path)).collect();
    app.folio.tabs.opening = Some(Opening { queue, waiting: None, active, unavailable: 0 });
}

fn open_next(app: &mut PdfCraftApp, opening: &mut Opening) -> bool {
    if let Some(waiting) = opening.waiting.as_deref() {
        if app.folio_tab_pending(waiting) {
            return true;
        }
        opening.waiting = None;
    }
    while let Some(path) = opening.queue.pop_front() {
        if !Path::new(&path).is_file() {
            opening.unavailable += 1;
            continue;
        }
        match app.open_path(&path) {
            OpenOutcome::Pending => {
                opening.waiting = Some(path);
                return true;
            }
            OpenOutcome::Failed => opening.unavailable += 1,
            OpenOutcome::Opened | OpenOutcome::Focused => {}
        }
    }
    if let Some(index) = opening.active.as_deref().and_then(|path| app.tab_showing(path)) {
        app.active = Some(index);
    }
    if opening.unavailable > 0 {
        app.notify(format!("Restored Folio tabs. {} files were unavailable.", opening.unavailable));
    } else {
        app.notify_tr("Restored Folio tabs");
    }
    false
}

fn text_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
