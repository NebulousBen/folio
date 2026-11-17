use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{ObjRef, Object, XrefEntry};
use pdfcraft_model::Page;

use super::{DocumentKey, PdfDocument, Reading, pdf_error};
use crate::{Document, EditError};

impl Document {
    fn folio_saved_password(&self) -> Option<&str> {
        self.editor.as_ref().and_then(|editor| editor.saved_keys.reopen.as_deref()).or(self.password.as_deref())
    }

    /// Move local reading anchors across an incremental save when the old page objects and
    /// their geometry-bearing content survived. A full rewrite can be a redaction or
    /// sanitization, so it cannot inherit local quotes or comments.
    pub fn rebase_folio_reading(&self, bytes: &Arc<Vec<u8>>, reading: &Reading) -> Result<Option<(DocumentKey, Reading)>, EditError> {
        let saved = self.saved_bytes();
        if saved.as_slice() == bytes.as_slice() || !bytes.starts_with(&saved) {
            return Ok(None);
        }
        let password = self.folio_saved_password();
        let old = PdfDocument::open_with_password(saved, password).map_err(pdf_error)?;
        let new = PdfDocument::open_with_password(Arc::clone(bytes), password).map_err(pdf_error)?;
        let previous = pdfcraft_model::pages(&old);
        let current = pdfcraft_model::pages(&new);
        let old_count = u32::try_from(previous.len()).map_err(pdf_error)?;
        let count = u32::try_from(current.len()).map_err(pdf_error)?;
        reading.validate(Some(old_count)).map_err(super::reading_error)?;
        let positions: HashMap<_, _> = current.iter().enumerate().map(|(index, page)| (page.obj, (index as u32 + 1, page))).collect();
        let mut unchanged = BTreeMap::new();
        for (index, page) in previous.iter().enumerate() {
            if let Some(&(number, target)) = positions.get(&page.obj)
                && same_anchors(&old, &new, page, target)
            {
                unchanged.insert(index as u32 + 1, number);
            }
        }
        if unchanged.is_empty() {
            return Ok(None);
        }
        let updated = reading.rebased(&unchanged);
        updated.validate(Some(count)).map_err(super::reading_error)?;
        Ok(Some((DocumentKey::of(bytes), updated)))
    }
}

fn same_anchors(old: &PdfDocument, new: &PdfDocument, before: &Page, after: &Page) -> bool {
    if before.crop(old) != after.crop(new) || before.user_unit(old) != after.user_unit(new) {
        return false;
    }
    let mut seen = HashSet::new();
    let mut work = 50_000;
    [b"Contents".as_slice(), b"Resources"].into_iter().all(|key| {
        let before = before.dict.get(key);
        let after = after.dict.get(key);
        before == after && before.is_none_or(|object| unchanged_object(old, new, object, &mut seen, &mut work, 0))
    })
}

fn unchanged_object(old: &PdfDocument, new: &PdfDocument, value: &Object, seen: &mut HashSet<ObjRef>, work: &mut usize, depth: usize) -> bool {
    if *work == 0 || depth > 64 {
        return false;
    }
    *work -= 1;
    let mut children_equal = |value| unchanged_object(old, new, value, seen, work, depth + 1);
    match value {
        Object::Ref(reference) => {
            if !seen.insert(*reference) {
                return true;
            }
            let entry = old.xref_entry(reference.num);
            if entry != new.xref_entry(reference.num) || !old.xref_entry_is_exact(reference.num) || !new.xref_entry_is_exact(reference.num) {
                return false;
            }
            if let Some(XrefEntry::InStream { stream, .. }) = entry
                && old.xref_entry(stream) != new.xref_entry(stream)
            {
                return false;
            }
            unchanged_object(old, new, &old.get(*reference), seen, work, depth + 1)
        }
        Object::Array(objects) => objects.iter().all(children_equal),
        Object::Dict(dict) => dict.iter().all(|(_, value)| children_equal(value)),
        Object::Stream(stream) => stream.dict.iter().all(|(_, value)| children_equal(value)),
        _ => true,
    }
}
