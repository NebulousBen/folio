use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use egui::{Color32, Stroke};
use pdfcraft_engine::folio::{AnnotationKind, Fit, Zoom, bracket_points, library};

use crate::PdfCraftApp;
use crate::canvas::PageXform;

pub(crate) struct Overlay {
    bytes: Arc<Vec<u8>>,
    pages: BTreeMap<usize, Vec<Mark>>,
}

struct Mark {
    kind: AnnotationKind,
    colour: [u8; 3],
    polygons: Vec<Vec<[f64; 2]>>,
}

pub(crate) fn restore_view(app: &mut PdfCraftApp, index: usize) {
    if !app.restore_last_view {
        return;
    }
    let Some(dir) = app.folio.directory.as_deref() else { return };
    let Some(view) = app.views.get_mut(index) else { return };
    let Some(doc) = app.session.get(view.id) else { return };
    if let Some(path) = doc.path.as_deref()
        && app.last_views.iter().any(|last| crate::same_file(path, &last.path))
    {
        return;
    }
    let Ok(Some(state)) = library::load(dir, &doc.folio_key(), doc.info.pages.len() as u32) else { return };
    if !app.always_default_view {
        match state.zoom {
            Zoom::Fit(Fit::Width) => view.set_fit(crate::canvas::Fit::Width),
            Zoom::Fit(Fit::Page) => view.set_fit(crate::canvas::Fit::Page),
            Zoom::Ratio(ratio) => view.set_zoom(ratio as f32),
        }
        view.rotation = state.rotation;
    }
    view.go_to_reading_position((state.page - 1) as usize, state.position.map(|position| [position.x as f32, position.y as f32]));
}

pub(crate) fn refresh(app: &mut PdfCraftApp, index: usize) {
    let Some(dir) = app.folio.directory.as_deref() else { return };
    let Some(view) = app.views.get_mut(index) else { return };
    let Some(doc) = app.session.get(view.id) else { return };
    let bytes = doc.saved_bytes();
    if doc.dirty || view.folio_overlay.as_ref().is_some_and(|overlay| Arc::ptr_eq(&overlay.bytes, &bytes)) {
        return;
    }
    let mut overlay = Overlay { bytes, pages: BTreeMap::new() };
    let state = library::load(dir, &doc.folio_key(), doc.info.pages.len() as u32);
    if let Ok(Some(reading)) = state
        && doc.check_copying().is_ok()
    {
        // Annotated copies already carry actual PDF annotations. Skip those marks in the overlay.
        let embedded = doc.folio_reading().ok().flatten();
        let materialized: HashSet<_> = embedded.iter().flat_map(|r| r.visible_annotations()).map(|mark| &mark.id).collect();
        for mark in reading.visible_annotations().filter(|mark| !materialized.contains(&mark.id)) {
            let index = (mark.page - 1) as usize;
            let Some(page) = doc.info.pages.get(index) else { continue };
            let polygons = if mark.kind == AnnotationKind::Section {
                vec![bracket_points(mark, page)]
            } else {
                mark.quads.iter().map(|q| vec![[q[0], q[1]], [q[2], q[3]], [q[6], q[7]], [q[4], q[5]]]).collect()
            };
            overlay.pages.entry(index).or_default().push(Mark { kind: mark.kind, colour: reading.colour(&mark.color), polygons });
        }
    }
    view.folio_overlay = Some(overlay);
}

impl Overlay {
    pub(crate) fn paint(&self, painter: &egui::Painter, index: usize, xform: &PageXform, page: &pdfcraft_render::PageInfo) {
        let Some(marks) = self.pages.get(&index) else { return };
        let painter = painter.with_clip_rect(xform.rect);
        for mark in marks {
            let [r, g, b] = mark.colour;
            let colour = Color32::from_rgb(r, g, b);
            let screen = |&[x, y]: &[f64; 2]| {
                let [x, y] = page.user_to_view(x as f32, y as f32);
                xform.norm_to_screen(x / xform.pw, y / xform.ph)
            };
            match mark.kind {
                AnnotationKind::Section => {
                    for line in &mark.polygons {
                        painter.add(egui::Shape::line(line.iter().map(screen).collect(), Stroke::new(2.0, colour)));
                    }
                }
                AnnotationKind::Highlight => {
                    let fill = Color32::from_rgba_unmultiplied(r, g, b, 90);
                    for polygon in &mark.polygons {
                        painter.add(egui::Shape::convex_polygon(polygon.iter().map(screen).collect(), fill, Stroke::NONE));
                    }
                }
                AnnotationKind::Note => {
                    if let Some(anchor) = mark.polygons.first().and_then(|polygon| polygon.first()) {
                        painter.circle_filled(screen(anchor), 4.0, colour);
                    }
                }
            }
        }
    }
}
