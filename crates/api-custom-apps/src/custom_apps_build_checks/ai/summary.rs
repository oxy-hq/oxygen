//! The build's summary: three or four lines on what the build changes
//! against production and where the risk is, kept as the attempt's `summary`
//! row (`dto::SUMMARY_KIND`) and answered as the top-level `summary` of `GET
//! …/checks` and as `checks.summary` of a promote plan.
//!
//! Written from two things only: the promote plan's own diff of the build
//! against what production serves (`custom_apps_promote::production_diff`:
//! functions, schedules, migrations) and the attempt's checks as they ended.
//! The row's `detail` is the note itself and its `notes` hold the same note,
//! so both readers answer the same object.

use chrono::Utc;
use entity::app_build_checks;
use sea_orm::DbErr;

use super::super::dto::SUMMARY_KIND;
use super::super::kind::{self, CheckKind, CheckStatus};
use super::inputs;
use super::model::NoteModel;
use super::notes::{self, MAX_SUMMARY_CHARS, ModelRow};
use super::prompts::{self, data, quoted};
use super::run::{Writing, did_not_pass};
use crate::custom_apps_promote::production_diff::{self, ProductionDiff};

/// The most names of one list the summary is given.
const MAX_NAMES: usize = 40;

/// `names`, comma separated, with what was left out counted; `none` for none.
fn listed(names: impl IntoIterator<Item = String>) -> String {
    let names: Vec<String> = names.into_iter().collect();
    if names.is_empty() {
        return "none".to_string();
    }
    let mut said = names
        .iter()
        .take(MAX_NAMES)
        .map(|name| kind::cap(name, 120))
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX_NAMES {
        said.push_str(&format!(" and {} more", names.len() - MAX_NAMES));
    }
    said
}

/// The checks of an attempt that did not pass, by label in list order, or
/// `none`. Stated outright rather than left to be read off the list: a small
/// model read a changed function named `health` as a health check that
/// failed, on an attempt where every check passed.
pub fn not_passed_line(rows: &[app_build_checks::Model]) -> String {
    let mut failed: Vec<_> = rows.iter().filter(|row| did_not_pass(row)).collect();
    failed.sort_by(|a, b| CheckKind::list_key(&a.kind).cmp(&CheckKind::list_key(&b.kind)));
    listed(failed.iter().map(|row| CheckKind::label_of(&row.kind)))
}

/// The plan's diff as the lines the model reads.
pub fn diff_lines(diff: &ProductionDiff) -> String {
    let mut lines = Vec::new();
    if let Some(functions) = &diff.functions {
        lines.push(format!(
            "functions added: {}; removed: {}; changed: {}",
            listed(functions.added.clone()),
            listed(functions.removed.clone()),
            listed(functions.changed.clone())
        ));
    }
    if let Some(schedules) = &diff.schedules {
        let cadence = |c: &crate::custom_apps_promote::schedules::Cadence| {
            format!("{} ({} {})", c.function, c.cron, c.timezone)
        };
        lines.push(format!(
            "schedules added: {}; removed: {}; changed: {}",
            listed(schedules.added.iter().map(cadence)),
            listed(schedules.removed.iter().map(cadence)),
            listed(schedules.changed.iter().map(|c| cadence(&c.to)))
        ));
    }
    if let Some(migrations) = &diff.migrations {
        lines.push(format!(
            "migrations applied to production before the move: transactional store: {}; \
             warehouse: {}",
            listed(migrations.oltp.clone()),
            listed(migrations.airhouse.clone())
        ));
    }
    for part in &diff.unread {
        lines.push(format!("{part}: could not be read"));
    }
    lines.join("\n")
}

impl Writing<'_> {
    /// The attempt's checks as the lines the model reads: each one's label,
    /// how it ended and its one-line summary.
    fn check_lines(&self) -> String {
        let mut rows: Vec<_> = self
            .rows
            .iter()
            .filter(|row| !kind::is_models(&row.kind))
            .collect();
        rows.sort_by(|a, b| CheckKind::list_key(&a.kind).cmp(&CheckKind::list_key(&b.kind)));
        rows.iter()
            .map(|row| {
                format!(
                    "{} [{}]: {}",
                    CheckKind::label_of(&row.kind),
                    row.status,
                    row.summary.as_deref().unwrap_or("(no summary)")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The screens the comparison found changed, by name.
    fn changed_lines(&self) -> String {
        let detail = self
            .row(CheckKind::Changes.as_str())
            .and_then(|row| row.detail.as_ref());
        let screens = detail.map(inputs::changed_screens).unwrap_or_default();
        listed(
            screens
                .iter()
                .map(|s| format!("{} ({})", s.named(), s.differs())),
        )
    }

    /// What the model is given for the summary.
    fn summary_text(&self, diff: &ProductionDiff) -> String {
        let serving = match &diff.current {
            Some(current) => format!("build {}", quoted(&format!("{:?}", current.build_id))),
            None => "nothing yet".to_string(),
        };
        [
            format!(
                "The build: {}. Production serves: {serving}.",
                quoted(&format!("{:?}", self.build.build_id))
            ),
            data(
                "what the build changes against production",
                &diff_lines(diff),
            ),
            data("the build's checks on staging", &self.check_lines()),
            data(
                "every check that did not pass (the whole list)",
                &not_passed_line(self.rows),
            ),
            data("screens that differ from production", &self.changed_lines()),
            "Write the summary.".to_string(),
        ]
        .join("\n")
    }

    /// Write the `summary` row: `text` as its note, by `model` when one
    /// wrote it.
    pub async fn put_summary(
        &self,
        model: Option<&str>,
        status: CheckStatus,
        text: &str,
    ) -> Result<(), DbErr> {
        let note = notes::note(model, text, MAX_SUMMARY_CHARS, Utc::now());
        let line = match status {
            CheckStatus::Passed => "the build's summary".to_string(),
            _ => text.to_string(),
        };
        let row = ModelRow {
            kind: SUMMARY_KIND,
            status,
            summary: line,
            detail: note.clone(),
            notes: vec![note],
            run_id: &self.run_id,
            started_at: self.started,
        };
        notes::put_row(self.db, self.app.id, self.key, row).await
    }

    /// The build's summary. What it did, for the run's answer.
    pub(super) async fn summary(&self, model: &dyn NoteModel) -> Result<&'static str, DbErr> {
        let diff = production_diff::read(self.db, self.app, self.build).await;
        let text = self.summary_text(&diff);
        match model.ask(&prompts::summary(), &text, &[]).await {
            Ok(answer) => {
                self.put_summary(Some(model.name()), CheckStatus::Passed, &answer)
                    .await?;
                Ok("written")
            }
            Err(e) => {
                self.log_failed(SUMMARY_KIND, e);
                let said = format!("No summary was written: {}.", e.said());
                self.put_summary(Some(model.name()), CheckStatus::Skipped, &said)
                    .await?;
                Ok("the model did not answer")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::custom_apps_promote::functions_diff::FunctionChanges;
    use crate::custom_apps_promote::plan::MigrationPlan;
    use crate::custom_apps_promote::schedules::{Cadence, ScheduleChanges};

    use super::*;

    fn row(kind: &str, status: &str) -> app_build_checks::Model {
        let at = chrono::Utc::now().fixed_offset();
        app_build_checks::Model {
            id: uuid::Uuid::new_v4(),
            app_id: uuid::Uuid::from_u128(1),
            build_id: uuid::Uuid::from_u128(10),
            environment: "staging".into(),
            kind: kind.into(),
            attempt: 1,
            status: status.into(),
            advisory: false,
            summary: None,
            detail: None,
            notes: serde_json::json!([]),
            run_id: None,
            runner: None,
            created_at: at,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn the_checks_that_did_not_pass_are_stated_outright_and_only_those() {
        let passed = [
            row("validation", "passed"),
            row("functions", "passed"),
            row("smoke", "passed"),
            row("changes", "passed"),
        ];
        assert_eq!(not_passed_line(&passed), "none");
        let some = [
            row("design_system", "error"),
            row("loads", "passed"),
            row("functions", "failed"),
            // A model's own row is no check, whatever it says.
            row(super::super::screens::DESIGN_AUDIT_KIND, "failed"),
        ];
        assert_eq!(
            not_passed_line(&some),
            "Its checks pass, Follows the design system"
        );
    }

    #[test]
    fn the_plans_diff_reads_a_line_a_part_and_names_what_could_not_be_read() {
        let cadence = |function: &str| Cadence {
            function: function.into(),
            cron: "0 6 * * *".into(),
            timezone: "UTC".into(),
        };
        let diff = ProductionDiff {
            current: None,
            functions: Some(FunctionChanges {
                added: vec!["reorder".into()],
                removed: Vec::new(),
                changed: vec!["sync".into(), "close".into()],
            }),
            schedules: Some(ScheduleChanges {
                added: Vec::new(),
                removed: vec![cadence("nightly")],
                changed: Vec::new(),
            }),
            migrations: None,
            unread: vec!["migrations"],
        };
        assert_eq!(
            diff_lines(&diff),
            "functions added: reorder; removed: none; changed: sync, close\n\
             schedules added: none; removed: nightly (0 6 * * * UTC); changed: none\n\
             migrations: could not be read"
        );
        let with = ProductionDiff {
            migrations: Some(MigrationPlan {
                when: "before_move",
                oltp: vec!["0002_orders.sql".into()],
                airhouse: Vec::new(),
            }),
            unread: Vec::new(),
            ..diff
        };
        assert!(diff_lines(&with).ends_with(
            "migrations applied to production before the move: transactional store: \
             0002_orders.sql; warehouse: none"
        ));
        let many = listed((0..45).map(|n| format!("f{n}")));
        assert!(many.ends_with("f39 and 5 more"), "{many}");
    }
}
