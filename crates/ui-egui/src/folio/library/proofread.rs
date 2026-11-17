use harper_core::linting::{Lint, LintGroup, Linter as _};
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};

use super::{Job, PdfCraftApp, finished, run};

#[derive(Default)]
pub(super) struct Checker {
    job: Option<Job<(String, Vec<Lint>)>>,
    result: Option<(String, Vec<Lint>)>,
}

impl Checker {
    pub(super) fn show(&mut self, app: &mut PdfCraftApp, ui: &mut egui::Ui, comment: &mut String) {
        if let Some(result) = finished(&mut self.job) {
            match result {
                Ok(result) => self.result = Some(result),
                Err(error) => app.notify_error(error),
            }
        }
        let eligible = self.job.is_none() && !comment.trim().is_empty() && comment.len() <= 64 * 1024;
        if ui.add_enabled(eligible, egui::Button::new("Check Canadian spelling and grammar")).clicked() {
            let text = comment.clone();
            self.result = None;
            match run("folio-proofreading", ui.ctx(), move || {
                let mut checker = LintGroup::new_curated(FstDictionary::curated(), Dialect::Canadian);
                let document = Document::new_plain_english_curated(&text);
                let mut issues = checker.lint(&document);
                issues.sort_by_key(|lint| (lint.span.start, lint.span.end));
                issues.truncate(200);
                Ok((text, issues))
            }) {
                Ok(job) => self.job = Some(job),
                Err(error) => app.notify_error(error),
            }
        }
        if self.job.is_some() {
            ui.spinner();
        }
        let Some((source, issues)) = self.result.as_ref() else { return };
        if source != comment {
            self.result = None;
            return;
        }
        if issues.is_empty() {
            ui.small("No spelling or grammar suggestions");
            return;
        }
        let source_chars: Vec<_> = source.chars().collect();
        let mut correction = None;
        egui::ScrollArea::vertical().id_salt("folio_proofreading").max_height(130.0).show(ui, |ui| {
            for issue in issues {
                let Some(problem) = source_chars.get(issue.span.start..issue.span.end) else { continue };
                ui.label(format!("{} · {}", problem.iter().collect::<String>(), issue.message));
                ui.horizontal_wrapped(|ui| {
                    for suggestion in issue.suggestions.iter().take(5) {
                        if ui.small_button(suggestion.to_string()).clicked() {
                            let mut chars = source_chars.clone();
                            suggestion.apply(issue.span, &mut chars);
                            correction = Some(chars.into_iter().collect());
                        }
                    }
                });
            }
        });
        if let Some(updated) = correction {
            *comment = updated;
            self.result = None;
        }
    }
}
