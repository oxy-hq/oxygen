//! A model's note, and the only three writes the notes task makes.
//!
//! **A note** is `{author: "model", model, text, refs: [], created_at}`: the
//! shape every reader of `app_build_checks.notes` already draws. `text` is the
//! model's answer, trimmed and capped, stored as text and read back as
//! nothing else.
//!
//! **The writes.** [`set_note`] touches one column, `notes`, of a check's row:
//! never its status, summary or detail. [`put_row`] writes a whole row, and
//! only of a model's kind (`kind::MODEL_KINDS`), always `advisory`. Both
//! replace what an earlier run of the same task wrote, so running it twice
//! leaves one note and one row.

use chrono::{DateTime, SecondsFormat, Utc};
use entity::app_build_checks::{self, Column, Entity};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ActiveValue, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::kind::{self, CheckStatus, cap};
use super::super::store::AttemptKey;

/// The `author` of every note this task writes.
pub const AUTHOR: &str = "model";
/// The longest note on one check, and the longest design audit.
pub const MAX_NOTE_CHARS: usize = 2_000;
/// The longest note on the changed screens: it can be several answers joined.
pub const MAX_CHANGES_NOTE_CHARS: usize = 4_000;
/// The longest summary: three or four lines.
pub const MAX_SUMMARY_CHARS: usize = 1_000;

/// One note: `text` (trimmed, cut to `max` characters) by `model`, at `at`.
/// `model` is `None` only where no model wrote: the two rows of a task that
/// found none.
pub fn note(model: Option<&str>, text: &str, max: usize, at: DateTime<Utc>) -> Value {
    json!({
        "author": AUTHOR,
        "model": model,
        "text": cap(text.trim(), max),
        "refs": [],
        "created_at": at.to_rfc3339_opts(SecondsFormat::Secs, true),
    })
}

/// Put `note` on the check `row_id`, in place of any note a model left there
/// before. Notes by anyone else stay, in their order. `true` when the row
/// took it.
pub async fn set_note<C: ConnectionTrait>(c: &C, row_id: Uuid, note: Value) -> Result<bool, DbErr> {
    let written = Entity::update_many()
        .col_expr(
            Column::Notes,
            Expr::cust_with_values(
                "(SELECT COALESCE(jsonb_agg(kept.note ORDER BY kept.at), '[]'::jsonb) \
                 FROM jsonb_array_elements(CASE WHEN jsonb_typeof(notes) = 'array' THEN notes \
                 ELSE '[]'::jsonb END) WITH ORDINALITY AS kept(note, at) \
                 WHERE kept.note->>'author' IS DISTINCT FROM 'model') || $1::jsonb",
                [Value::Array(vec![note])],
            ),
        )
        .filter(Column::Id.eq(row_id))
        .exec(c)
        .await?;
    Ok(written.rows_affected > 0)
}

/// One of the two rows a model writes, whole.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRow<'a> {
    /// `design_audit` or `summary`.
    pub kind: &'a str,
    /// `passed` (the model wrote) or `skipped` (it did not, and why is the
    /// summary).
    pub status: CheckStatus,
    /// One line.
    pub summary: String,
    /// The row's `detail`.
    pub detail: Value,
    /// The row's `notes`.
    pub notes: Vec<Value>,
    /// The task's run.
    pub run_id: &'a str,
    pub started_at: DateTime<Utc>,
}

/// `row` as it is stored for `key`'s attempt: always `advisory`, finished now.
fn stored(app_id: Uuid, key: &AttemptKey, row: ModelRow<'_>) -> app_build_checks::ActiveModel {
    let now = Utc::now().fixed_offset();
    app_build_checks::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        build_id: ActiveValue::Set(key.build_pk),
        environment: ActiveValue::Set(key.environment.clone()),
        kind: ActiveValue::Set(row.kind.to_string()),
        attempt: ActiveValue::Set(key.attempt),
        status: ActiveValue::Set(row.status.as_str().to_string()),
        advisory: ActiveValue::Set(true),
        summary: ActiveValue::Set(Some(cap(&row.summary, 200))),
        detail: ActiveValue::Set(Some(row.detail)),
        notes: ActiveValue::Set(Value::Array(row.notes)),
        run_id: ActiveValue::Set(Some(row.run_id.to_string())),
        runner: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        started_at: ActiveValue::Set(Some(row.started_at.fixed_offset())),
        finished_at: ActiveValue::Set(Some(now)),
    }
}

/// Write `row` for `key`'s attempt, replacing the row of that kind an earlier
/// run wrote. Refused for any kind that is not a model's: a check's row is
/// its runner's or the platform's, and this task never writes one.
pub async fn put_row<C: ConnectionTrait>(
    c: &C,
    app_id: Uuid,
    key: &AttemptKey,
    row: ModelRow<'_>,
) -> Result<(), DbErr> {
    if !kind::is_models(row.kind) {
        return Err(DbErr::Custom(format!(
            "the notes task writes no `{}` row: it is not a model's kind",
            row.kind
        )));
    }
    Entity::insert(stored(app_id, key, row))
        .on_conflict(
            OnConflict::columns([
                Column::BuildId,
                Column::Environment,
                Column::Kind,
                Column::Attempt,
            ])
            .update_columns([
                Column::Status,
                Column::Advisory,
                Column::Summary,
                Column::Detail,
                Column::Notes,
                Column::RunId,
                Column::StartedAt,
                Column::FinishedAt,
            ])
            .to_owned(),
        )
        .exec_without_returning(c)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn a_note_is_the_shape_every_reader_draws_and_is_capped() {
        let at = Utc.with_ymd_and_hms(2026, 11, 22, 10, 5, 0).unwrap();
        assert_eq!(
            note(
                Some("claude-sonnet-5-5"),
                "  The header moved.\n",
                MAX_NOTE_CHARS,
                at
            ),
            json!({ "author": "model", "model": "claude-sonnet-5-5",
                "text": "The header moved.", "refs": [], "created_at": "2026-11-22T10:05:00Z" })
        );
        let long = note(None, &"x".repeat(5_000), MAX_SUMMARY_CHARS, at);
        assert_eq!(long["text"].as_str().unwrap().chars().count(), 1_000);
        assert!(long["text"].as_str().unwrap().ends_with('…'));
        assert_eq!(long["model"], json!(null));
    }
}
