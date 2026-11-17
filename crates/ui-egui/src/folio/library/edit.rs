use pdfcraft_engine::folio::{Annotation, AnnotationKind, Bookmark, Category, CategoryKind, Layer, LayerKind, Reading, SectionRole};

use super::{Notes, PdfCraftApp, commit_reading};

pub(super) enum Editor {
    Annotation(Annotation, super::proofread::Checker),
    Bookmark(Bookmark),
    Category(Category),
    Layer(Layer, bool),
}

pub(super) fn tools(app: &mut PdfCraftApp, ui: &mut egui::Ui, notes: &mut Notes, page: u32, pages: u32) {
    ui.horizontal_wrapped(|ui| {
        ui.menu_button("Mark selection", |ui| {
            for (label, kind) in
                [("Highlight", AnnotationKind::Highlight), ("Reading note", AnnotationKind::Note), ("Margin bracket", AnnotationKind::Section)]
            {
                if ui.button(label).clicked() {
                    match annotation(app, notes, kind) {
                        Ok(mark) => notes.editor = Some(Editor::annotation(mark)),
                        Err(error) => app.notify_error(error),
                    }
                    ui.close();
                }
            }
        });
        if ui.button("Add bookmark…").clicked() {
            match id() {
                Ok(id) => {
                    notes.editor =
                        Some(Editor::Bookmark(Bookmark { id, page, label: format!("Page {page}"), end_page: None, show_label: Some(true) }))
                }
                Err(error) => app.notify_error(error),
            }
        }
        ui.menu_button("Manage layers", |ui| {
            if ui.button("New layer…").clicked() {
                match layer() {
                    Ok(layer) => notes.editor = Some(Editor::Layer(layer, false)),
                    Err(error) => app.notify_error(error),
                }
                ui.close();
            }
            for layer in &notes.reading.layers {
                if ui.button(&layer.title).clicked() {
                    let active = notes.reading.active_layer_id.as_deref() == Some(&layer.id);
                    notes.editor = Some(Editor::Layer(layer.clone(), active));
                    ui.close();
                }
            }
        });
        ui.menu_button("Manage categories", |ui| {
            if ui.button("New category…").clicked() {
                match id() {
                    Ok(id) => {
                        notes.editor = Some(Editor::Category(Category {
                            id,
                            hex: "#ffd54f".into(),
                            label: String::new(),
                            archived: Some(false),
                            meaning: None,
                            kind: Some(CategoryKind::Highlight),
                        }))
                    }
                    Err(error) => app.notify_error(error),
                }
                ui.close();
            }
            for category in &notes.reading.highlight_colors {
                if ui.button(&category.label).clicked() {
                    notes.editor = Some(Editor::Category(category.clone()));
                    ui.close();
                }
            }
        });
    });
    if let Some(mut editor) = notes.editor.take()
        && !editor.show(app, ui, notes, pages)
    {
        notes.editor = Some(editor);
    }
}

impl Editor {
    pub(super) fn annotation(mark: Annotation) -> Self {
        Self::Annotation(mark, super::proofread::Checker::default())
    }

    fn show(&mut self, app: &mut PdfCraftApp, ui: &mut egui::Ui, notes: &mut Notes, pages: u32) -> bool {
        let mut remove = false;
        let mut save = false;
        let mut close = false;
        ui.group(|ui| {
            match self {
                Self::Annotation(mark, checker) => {
                    edit_annotation(ui, &notes.reading, mark);
                    checker.show(app, ui, &mut mark.comment);
                }
                Self::Bookmark(bookmark) => edit_bookmark(ui, bookmark, pages),
                Self::Category(category) => edit_category(ui, category),
                Self::Layer(layer, active) => edit_layer(ui, layer, active),
            }
            ui.horizontal(|ui| {
                save = ui.button("Apply to reading").clicked();
                close = ui.button("Cancel").clicked();
                if matches!(self, Self::Annotation(mark, _) if notes.reading.annotations.iter().any(|row| row.id == mark.id))
                    || matches!(self, Self::Bookmark(bookmark) if notes.reading.bookmarks.iter().any(|row| row.id == bookmark.id))
                {
                    remove = ui.button("Remove").clicked();
                }
            });
        });
        if close || (!save && !remove) {
            return close;
        }
        let mut state = notes.reading.clone();
        match self {
            Self::Annotation(mark, _) => update(&mut state.annotations, mark, |row| &row.id, remove),
            Self::Bookmark(bookmark) => update(&mut state.bookmarks, bookmark, |row| &row.id, remove),
            Self::Category(category) => update(&mut state.highlight_colors, category, |row| &row.id, false),
            Self::Layer(layer, active) => {
                if *active {
                    state.active_layer_id = Some(layer.id.clone());
                } else if state.active_layer_id.as_deref() == Some(&layer.id) {
                    state.active_layer_id = None;
                }
                update(&mut state.layers, layer, |row| &row.id, false);
            }
        }
        match commit_reading(app, notes, state) {
            Ok(()) => {
                notes.selected_bookmarks.retain(|id| notes.reading.bookmarks.iter().any(|row| row.id == *id));
                true
            }
            Err(error) => {
                app.notify_error(error);
                false
            }
        }
    }
}

fn update<T: Clone>(rows: &mut Vec<T>, row: &T, id: impl Fn(&T) -> &str, remove: bool) {
    match (rows.iter().position(|existing| id(existing) == id(row)), remove) {
        (Some(index), true) => {
            rows.remove(index);
        }
        (Some(index), false) => rows[index] = row.clone(),
        (None, false) => rows.push(row.clone()),
        (None, true) => {}
    }
}

fn edit_annotation(ui: &mut egui::Ui, reading: &Reading, mark: &mut Annotation) {
    ui.strong(format!("Page {}", mark.page));
    if mark.kind == AnnotationKind::Section {
        ui.add(egui::TextEdit::singleline(mark.title.get_or_insert_with(String::new)).hint_text("Bracket title")).accessible_name("Bracket title");
        egui::ComboBox::from_label("Bracket role").selected_text(mark.section_role.map_or("Unassigned", role_label)).show_ui(ui, |ui| {
            ui.selectable_value(&mut mark.section_role, None, "Unassigned");
            for role in
                [SectionRole::Import, SectionRole::Export, SectionRole::Argument, SectionRole::Setup, SectionRole::Objection, SectionRole::Reply]
            {
                ui.selectable_value(&mut mark.section_role, Some(role), role_label(role));
            }
        });
        let mut argument = mark.argument_id.take().unwrap_or_default();
        ui.add(egui::TextEdit::singleline(&mut argument).hint_text("Argument ID")).accessible_name("Argument ID");
        mark.argument_id = (!argument.trim().is_empty()).then_some(argument);
    }
    egui::ComboBox::from_label("Mark category").selected_text(super::category_label(reading, &mark.color)).show_ui(ui, |ui| {
        for colour in ["yellow", "green", "blue", "pink", "purple"] {
            ui.selectable_value(&mut mark.color, colour.to_owned(), colour);
        }
        for category in &reading.highlight_colors {
            let section = category.kind == Some(CategoryKind::Section);
            if category.archived != Some(true) && (category.kind.is_none() || section == (mark.kind == AnnotationKind::Section)) {
                ui.selectable_value(&mut mark.color, category.id.clone(), &category.label);
            }
        }
    });
    let selected = reading.layers.iter().find(|layer| mark.layer_id.as_deref() == Some(&layer.id)).map_or("Personal reading", |layer| &layer.title);
    egui::ComboBox::from_label("Mark layer").selected_text(selected).show_ui(ui, |ui| {
        ui.selectable_value(&mut mark.layer_id, None, "Personal reading");
        for layer in &reading.layers {
            ui.selectable_value(&mut mark.layer_id, Some(layer.id.clone()), &layer.title);
        }
    });
    ui.label(&mark.quote);
    ui.add(egui::TextEdit::multiline(&mut mark.comment).hint_text("Comment").desired_rows(3)).accessible_name("Reading comment");
    ui.small(super::super::word_count(&mark.comment));
}

fn edit_bookmark(ui: &mut egui::Ui, bookmark: &mut Bookmark, pages: u32) {
    ui.add(egui::TextEdit::singleline(&mut bookmark.label).hint_text("Bookmark label")).accessible_name("Bookmark label");
    ui.add(egui::DragValue::new(&mut bookmark.page).range(1..=pages).prefix("Start page "));
    let mut range = bookmark.end_page.is_some();
    ui.checkbox(&mut range, "Bookmark page range");
    if range {
        let end = bookmark.end_page.get_or_insert(bookmark.page);
        *end = (*end).max(bookmark.page);
        ui.add(egui::DragValue::new(end).range(bookmark.page..=pages).prefix("End page "));
    } else {
        bookmark.end_page = None;
    }
}

fn edit_category(ui: &mut egui::Ui, category: &mut Category) {
    ui.add(egui::TextEdit::singleline(&mut category.label).hint_text("Category label")).accessible_name("Category label");
    ui.add(egui::TextEdit::singleline(&mut category.hex).hint_text("#ffd54f")).accessible_name("Category colour");
    ui.add(egui::TextEdit::singleline(category.meaning.get_or_insert_with(String::new)).hint_text("Meaning")).accessible_name("Category meaning");
    ui.checkbox(category.archived.get_or_insert(false), "Archive category");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut category.kind, Some(CategoryKind::Highlight), "Highlight");
        ui.selectable_value(&mut category.kind, Some(CategoryKind::Section), "Bracket");
    });
}

fn edit_layer(ui: &mut egui::Ui, layer: &mut Layer, active: &mut bool) {
    ui.add(egui::TextEdit::singleline(&mut layer.title).hint_text("Layer title")).accessible_name("Layer title");
    ui.add(egui::TextEdit::singleline(&mut layer.author).hint_text("Author")).accessible_name("Layer author");
    ui.checkbox(&mut layer.visible, "Show layer on pages");
    ui.checkbox(active, "Use layer for new marks");
}

fn annotation(app: &PdfCraftApp, notes: &Notes, kind: AnnotationKind) -> Result<Annotation, String> {
    let (index, id) = app.active_ids().filter(|&(_, id)| id == notes.document).ok_or("Open the PDF first")?;
    let doc = app.session.get(id).ok_or("This document was closed")?;
    let view = app.views.get(index).ok_or("This document was closed")?;
    let (page, quads, quote) = view
        .reading_selection(&doc.info, app.comment_prefs.smart_highlight && kind == AnnotationKind::Highlight)
        .ok_or("Select a passage on the PDF first")?;
    Ok(Annotation {
        id: self::id()?,
        page: page as u32 + 1,
        kind,
        color: "yellow".into(),
        quads,
        quote,
        comment: String::new(),
        created_at: now(),
        title: (kind == AnnotationKind::Section).then(String::new),
        argument_id: None,
        section_role: None,
        source_id: None,
        source_name: None,
        layer_id: notes.reading.active_layer_id.clone(),
        author: None,
        ai: None,
    })
}

fn layer() -> Result<Layer, String> {
    Ok(Layer {
        id: id()?,
        title: String::new(),
        author: "Personal".into(),
        visible: true,
        kind: LayerKind::Personal,
        run_id: None,
        import_key: None,
        color_ids: Vec::new(),
        created_at: Some(now()),
        purpose: None,
        standard_version: None,
        legend: Vec::new(),
        result_hash: None,
        source_key: None,
    })
}

pub(super) fn id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn now() -> String {
    chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).to_rfc3339()
}

fn role_label(role: SectionRole) -> &'static str {
    match role {
        SectionRole::Import => "Import",
        SectionRole::Export => "Export",
        SectionRole::Argument => "Argument",
        SectionRole::Setup => "Setup",
        SectionRole::Objection => "Objection",
        SectionRole::Reply => "Reply",
    }
}
