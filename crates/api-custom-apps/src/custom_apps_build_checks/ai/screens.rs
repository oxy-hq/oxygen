//! The two notes the model is shown pictures for: the changed screens sorted
//! into "Look here first" and "Looks intended" (on the `changes` row), and
//! the design audit (the `design_audit` row).
//!
//! Both read the changed screens the `changes` row lists. The first shows
//! each one's picture on staging and on production, with its share of
//! differing pixels and its visible text on each; the second shows each one's
//! picture on staging, with the build's CSS digest. How the pictures travel,
//! and what happens when the model does not take them, is [`super::visual`]'s.

use chrono::Utc;
use sea_orm::DbErr;
use serde_json::{Value, json};

use super::super::kind::{CheckKind, CheckStatus, MODEL_KINDS};
use super::inputs::{self, ChangedScreen, Files};
use super::model::NoteModel;
use super::notes::{self, MAX_CHANGES_NOTE_CHARS, MAX_NOTE_CHARS, ModelRow};
use super::prompts::{self, data, quoted};
use super::run::Writing;
use super::visual::{self, Asked, Question, Subject};

/// The kind of the design audit's row.
pub const DESIGN_AUDIT_KIND: &str = MODEL_KINDS[0];

/// An answer as a note's text: what the model was not shown first, when
/// there was something it was not shown.
fn with_unseen(asked: &Asked) -> String {
    match asked.unseen_line() {
        Some(line) => format!("{line}\n\n{}", asked.text),
        None => asked.text.clone(),
    }
}

/// One changed screen as the changes note's subject: its numbers, its two
/// texts and its two pictures.
async fn compared(files: &Files<'_>, screen: &ChangedScreen) -> Subject {
    let named = quoted(&screen.named());
    let none = "(no text was kept)".to_string();
    let staging = files.text(&screen.text_name("staging")).await;
    let production = files.text(&screen.text_name("production")).await;
    let words = [
        format!("About {named}: {}.", screen.differs()),
        data(
            "its text on staging (the new build)",
            &staging.unwrap_or(none.clone()),
        ),
        data(
            "its text on production (live now)",
            &production.unwrap_or(none),
        ),
    ]
    .join("\n");
    let shot = |name: &Option<String>, place: &str| {
        name.clone()
            .map(|name| (name, format!("Picture of {named}, on {place}:")))
    };
    let pictures = [
        shot(&screen.staging, "staging (the new build)"),
        shot(&screen.production, "production (live now)"),
    ];
    Subject {
        words,
        pictures: pictures.into_iter().flatten().collect(),
    }
}

/// One changed screen as the design audit's subject: its staging picture.
fn on_staging(screen: &ChangedScreen) -> Subject {
    let named = quoted(&screen.named());
    Subject {
        words: format!("A changed screen: {named}, on staging."),
        pictures: screen
            .staging
            .iter()
            .map(|name| (name.clone(), format!("Picture of {named}, on staging:")))
            .collect(),
    }
}

/// The design audit's one line: what the model was given.
fn audited(digest: bool, seen: usize, screens: usize) -> String {
    let pictures = format!("the pictures of {seen} of {screens} changed screens");
    match (digest, screens) {
        (true, 0) => "audited the CSS digest (no screen changed)".to_string(),
        (true, _) => format!("audited the CSS digest and {pictures}"),
        (false, _) => format!("audited {pictures} (no CSS digest was kept)"),
    }
}

impl Writing<'_> {
    /// The screens the attempt's `changes` check found changed; none when the
    /// comparison did not run to an answer.
    fn changed_screens(&self) -> Vec<ChangedScreen> {
        let compared = self
            .row(CheckKind::Changes.as_str())
            .filter(|row| row.status == CheckStatus::Passed.as_str());
        compared
            .and_then(|row| row.detail.as_ref())
            .map(inputs::changed_screens)
            .unwrap_or_default()
    }

    /// Write the `design_audit` row.
    pub async fn put_design_audit(
        &self,
        status: CheckStatus,
        summary: String,
        notes: Vec<Value>,
    ) -> Result<(), DbErr> {
        let row = ModelRow {
            kind: DESIGN_AUDIT_KIND,
            status,
            summary,
            detail: json!({ "findings": [] }),
            notes,
            run_id: &self.run_id,
            started_at: self.started,
        };
        notes::put_row(self.db, self.app.id, self.key, row).await
    }

    /// The note on the `changes` row. What it did, for the run's answer.
    async fn changes_note(
        &self,
        model: &dyn NoteModel,
        files: &Files<'_>,
        screens: &[ChangedScreen],
    ) -> Result<&'static str, DbErr> {
        let Some(row) = self
            .row(CheckKind::Changes.as_str())
            .filter(|_| !screens.is_empty())
        else {
            return Ok("nothing changed, no note");
        };
        let mut subjects = Vec::with_capacity(screens.len());
        for screen in screens {
            subjects.push(compared(files, screen).await);
        }
        let system = prompts::changes();
        let opening = format!(
            "{} screens of this build differ from production. Sort them.",
            screens.len()
        );
        let question = Question {
            system: &system,
            opening: &opening,
            continuing: "More changed screens of the same build. Sort these too.",
            subjects: &subjects,
        };
        let (said, did) = match visual::ask(model, files, question).await {
            Ok(asked) => (with_unseen(&asked), "note written"),
            Err(e) => {
                self.log_failed("changes", e);
                let said = format!(
                    "No note was written on the changed screens: {}. The comparison itself is \
                     as its runner left it.",
                    e.said()
                );
                (said, "the model did not answer")
            }
        };
        let note = notes::note(
            Some(model.name()),
            &said,
            MAX_CHANGES_NOTE_CHARS,
            Utc::now(),
        );
        notes::set_note(self.db, row.id, note).await?;
        Ok(did)
    }

    /// The `design_audit` row. What it did, for the run's answer.
    async fn design_audit(
        &self,
        model: &dyn NoteModel,
        files: &Files<'_>,
        screens: &[ChangedScreen],
    ) -> Result<&'static str, DbErr> {
        let digest = files.css_digest().await;
        let subjects: Vec<Subject> = screens.iter().map(on_staging).collect();
        let pictured = subjects.iter().any(|s| !s.pictures.is_empty());
        let shown = pictured && model.image_limits().is_some();
        if digest.is_none() && !shown {
            let why = "not audited: the attempt kept no CSS digest, and no changed screen's \
                       picture this workspace's model can be shown";
            self.put_design_audit(CheckStatus::Skipped, why.to_string(), Vec::new())
                .await?;
            return Ok("skipped, nothing to audit");
        }
        let css = digest
            .as_deref()
            .unwrap_or("(the attempt kept no CSS digest)");
        let opening = [
            data("the app's own CSS: file | selector | declarations", css),
            format!("{} changed screens of the build follow.", subjects.len()),
        ]
        .join("\n");
        let system = prompts::design_audit();
        let question = Question {
            system: &system,
            opening: &opening,
            continuing: "More screens of the same build. Report only what these show: the CSS \
                         was covered before.",
            subjects: &subjects,
        };
        match visual::ask(model, files, question).await {
            Ok(asked) => {
                let summary = audited(digest.is_some(), asked.seen, subjects.len());
                let said = with_unseen(&asked);
                let note = notes::note(Some(model.name()), &said, MAX_NOTE_CHARS, Utc::now());
                self.put_design_audit(CheckStatus::Passed, summary, vec![note])
                    .await?;
                Ok("written")
            }
            Err(e) => {
                self.log_failed(DESIGN_AUDIT_KIND, e);
                let why = format!("not audited: {}", e.said());
                self.put_design_audit(CheckStatus::Skipped, why, Vec::new())
                    .await?;
                Ok("the model did not answer")
            }
        }
    }

    /// The two notes the model is shown pictures for. What each did.
    pub(super) async fn screens(
        &self,
        model: &dyn NoteModel,
    ) -> Result<(&'static str, &'static str), DbErr> {
        let screens = self.changed_screens();
        let Some(first) = self.rows.first() else {
            return Ok(("no rows", "no rows"));
        };
        let files = Files::of(first);
        let changes = self.changes_note(model, &files, &screens).await?;
        let audit = self.design_audit(model, &files, &screens).await?;
        Ok((changes, audit))
    }
}
