use std::collections::{BTreeMap, HashMap};

use pdfcraft_cos::ObjRef;
use pdfcraft_folio_reading::FieldValue as LegacyValue;
use pdfcraft_forms::{Field, FieldKind, FieldValue, flags};

use super::{Document, Reading};
use crate::Edit;

pub struct FormRestore {
    pub edits: Vec<Edit>,
    pub skipped: usize,
}

impl Document {
    /// Map the old reader's PDF.js widget references to the native form tree in these PDF bytes.
    pub fn folio_form_restore(&self, reading: &Reading) -> FormRestore {
        let mut references = HashMap::new();
        for (field_index, field) in self.form.iter().enumerate() {
            references.entry(field.obj).or_insert((field_index, None));
            for (widget_index, widget) in field.widgets.iter().enumerate().filter(|(_, widget)| !widget.read_only && !widget.hidden) {
                references.insert(widget.obj, (field_index, Some(widget_index)));
            }
        }
        let mut candidates: BTreeMap<usize, Vec<(Option<usize>, &LegacyValue)>> = BTreeMap::new();
        let mut skipped = 0;
        for (reference, saved) in &reading.form_values {
            match parse_reference(reference).and_then(|reference| references.get(&reference)) {
                Some(&(field, widget)) => candidates.entry(field).or_default().push((widget, &saved.value)),
                None => skipped += 1,
            }
        }
        let mut by_name: BTreeMap<String, (FieldValue, usize)> = BTreeMap::new();
        let mut ambiguous = std::collections::BTreeSet::new();
        for (index, values) in candidates {
            let field = &self.form[index];
            if field.read_only() || field.has(flags::PASSWORD | flags::FILE_SELECT) && field.kind == FieldKind::Text {
                skipped += values.len();
                continue;
            }
            if let Some(value) = native_value(field, &values) {
                match by_name.entry(field.name.clone()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert((value, values.len()));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().0 != value {
                            ambiguous.insert(field.name.clone());
                        }
                        entry.get_mut().1 += values.len();
                    }
                }
            } else {
                skipped += values.len();
            }
        }
        let edits = by_name
            .into_iter()
            .filter_map(|(name, (value, count))| {
                if ambiguous.contains(&name) {
                    skipped += count;
                    None
                } else {
                    Some(Edit::SetFieldValue { name, value })
                }
            })
            .collect();
        FormRestore { edits, skipped }
    }
}

fn parse_reference(text: &str) -> Option<ObjRef> {
    let (number, generation) = text.split_once('R')?;
    let number = number.parse::<u32>().ok().filter(|&n| n > 0)?;
    let generation = if generation.is_empty() { 0 } else { generation.parse().ok()? };
    Some(ObjRef::new(number, generation))
}

fn native_value(field: &Field, values: &[(Option<usize>, &LegacyValue)]) -> Option<FieldValue> {
    let (_, first) = values.first()?;
    match field.kind {
        FieldKind::CheckBox | FieldKind::Radio => button_value(field, values),
        FieldKind::Text | FieldKind::Combo | FieldKind::List if values.iter().all(|(_, value)| value == first) => match (field.kind, first) {
            (FieldKind::Text, LegacyValue::Text(value)) => Some(FieldValue::Text(value.clone())),
            (FieldKind::Combo | FieldKind::List, LegacyValue::Text(value)) => Some(FieldValue::Choice(vec![value.clone()])),
            (FieldKind::Combo | FieldKind::List, LegacyValue::Multiple(values)) => Some(FieldValue::Choice(values.clone())),
            _ => None,
        },
        _ => None,
    }
}

fn button_value(field: &Field, values: &[(Option<usize>, &LegacyValue)]) -> Option<FieldValue> {
    let selected: Vec<_> = values.iter().filter(|(_, value)| matches!(value, LegacyValue::Toggle(true))).collect();
    match selected.as_slice() {
        [(Some(widget), _)] => field.widgets.get(*widget)?.on_state.as_ref().map(|_| FieldValue::Widget(*widget)),
        [(None, _)] if field.kind == FieldKind::CheckBox => Some(FieldValue::Check(true)),
        [] if values.iter().all(|(_, value)| matches!(value, LegacyValue::Toggle(false))) => {
            if field.kind == FieldKind::CheckBox {
                Some(FieldValue::Check(false))
            } else {
                Some(FieldValue::Radio(None))
            }
        }
        _ => None,
    }
}
