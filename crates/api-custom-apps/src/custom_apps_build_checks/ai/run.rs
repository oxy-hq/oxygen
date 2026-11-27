//! One run of the notes task: what it reads, the order it writes in, and
//! what it does when there is no model or a call fails ([`super`] has the
//! rules).
//!
//! In order ([`Writing::all`]): a note on each check that did not pass; the
//! note on the changed screens and the design audit ([`super::screens`]); the
//! build's summary ([`super::summary`]). Each is written as soon as it is
//! known, so a run that dies half way leaves what it had, and the next run
//! replaces it.

use chrono::{DateTime, Utc};
use entity::{app_build_checks, app_builds, apps};
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};

use super::super::dto::SUMMARY_KIND;
use super::super::kind::{self, CheckKind, CheckStatus};
use super::super::store::{self, AttemptKey};
use super::super::{settings, stalled};
use super::model::{AskError, NoteModel};
use super::notes::{self, MAX_NOTE_CHARS};
use super::prompts;
use super::task::BuildChecksAiTask;

/// What one run has in hand: the attempt as it settled.
pub struct Writing<'a> {
    pub db: &'a DatabaseConnection,
    pub app: &'a apps::Model,
    pub build: &'a app_builds::Model,
    pub key: &'a AttemptKey,
    /// Every row of the attempt.
    pub rows: &'a [app_build_checks::Model],
    /// The task's run, named on the rows it writes.
    pub run_id: String,
    pub started: DateTime<Utc>,
}

/// Whether a row is a check that ended `failed` or `error`.
pub(super) fn did_not_pass(row: &app_build_checks::Model) -> bool {
    let ended = CheckStatus::parse(&row.status);
    !kind::is_models(&row.kind) && matches!(ended, Some(CheckStatus::Failed | CheckStatus::Error))
}

/// Whether an earlier run already wrote this attempt's notes: the summary is
/// written last, so its row is there.
fn already_written(rows: &[app_build_checks::Model]) -> bool {
    rows.iter()
        .any(|row| row.kind == SUMMARY_KIND && !stalled::is_open(row))
}

impl Writing<'_> {
    /// The attempt's row of `kind`.
    pub fn row(&self, kind: &str) -> Option<&app_build_checks::Model> {
        self.rows.iter().find(|row| row.kind == kind)
    }

    /// A model call failed: said in the log by what it was for and its class,
    /// and nothing of what was asked.
    pub fn log_failed(&self, what: &str, error: AskError) {
        tracing::warn!(app_id = %self.app.id, build = %self.build.build_id,
            attempt = self.key.attempt, what, error = ?error,
            "build checks notes: the model did not answer");
    }

    /// One note on each check that did not pass. A call that fails leaves a
    /// note saying so; the check's row is otherwise untouched either way.
    async fn failed_checks(&self, model: &dyn NoteModel) -> Result<usize, DbErr> {
        let system = prompts::failed_check();
        let mut written = 0;
        for row in self.rows.iter().filter(|row| did_not_pass(row)) {
            let label = CheckKind::label_of(&row.kind);
            let summary = row.summary.as_deref().unwrap_or("(none)");
            let text =
                prompts::failed_check_text(&label, &row.status, summary, row.detail.as_ref());
            let said = match model.ask(&system, &text, &[]).await {
                Ok(answer) => answer,
                Err(e) => {
                    self.log_failed(&row.kind, e);
                    format!(
                        "No note was written on this check: {}. The check's result is as its \
                         runner left it.",
                        e.said()
                    )
                }
            };
            let note = notes::note(Some(model.name()), &said, MAX_NOTE_CHARS, Utc::now());
            notes::set_note(self.db, row.id, note).await?;
            written += 1;
        }
        Ok(written)
    }

    /// Everything, with `model`. The run's answer.
    async fn all(&self, model: &dyn NoteModel) -> Result<String, DbErr> {
        let checks = self.failed_checks(model).await?;
        let (changes, audit) = self.screens(model).await?;
        let summary = self.summary(model).await?;
        Ok(format!(
            "{checks} notes on checks that did not pass; changed screens: {changes}; design \
             audit: {audit}; summary: {summary}"
        ))
    }

    /// No model to write with: the two rows say so, and no note is written.
    async fn skipped(&self, why: &str) -> Result<String, DbErr> {
        self.put_design_audit(
            CheckStatus::Skipped,
            format!("not audited: {why}"),
            Vec::new(),
        )
        .await?;
        let said = format!("No summary was written: {why}.");
        self.put_summary(None, CheckStatus::Skipped, &said).await?;
        Ok(format!("no notes written: {why}"))
    }
}

/// Write the notes of `task`'s attempt with `model`, or, with `Err(why)`
/// (the app's workspace has no model to write with), its two rows as
/// `skipped`. `Ok` is the run's answer; `Err` is a database failure.
pub async fn run(
    db: &DatabaseConnection,
    task: &BuildChecksAiTask,
    model: Result<&dyn NoteModel, String>,
) -> Result<String, String> {
    let db_err = |e: DbErr| format!("the database could not be read or written ({e})");
    let app = apps::Entity::find_by_id(task.app_id)
        .one(db)
        .await
        .map_err(db_err)?;
    let build = app_builds::Entity::find_by_id(task.build_pk)
        .one(db)
        .await
        .map_err(db_err)?;
    let (Some(app), Some(build)) = (app, build) else {
        return Ok("the app or the build is gone; nothing to write".to_string());
    };
    if !settings::enabled_for(app.org_id) {
        return Ok("the promote checks gate is off for the app's org; nothing written".to_string());
    }
    let key = task.key();
    let rows = store::rows_of(db, &key).await.map_err(db_err)?;
    if !store::is_settled(&rows) {
        return Ok("the attempt has no settled checks; nothing written".to_string());
    }
    if already_written(&rows) {
        return Ok("the attempt's notes are already written".to_string());
    }
    let writing = Writing {
        db,
        app: &app,
        build: &build,
        key: &key,
        rows: &rows,
        run_id: task.run_id(),
        started: Utc::now(),
    };
    let written = match model {
        Ok(model) => writing.all(model).await,
        Err(why) => writing.skipped(&why).await,
    };
    written.map_err(db_err)
}
