//! CRUD on the `agentic_run_suspensions` table.

use agentic_core::human_input::SuspendedRunData;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::*, DatabaseConnection, DbErr, EntityTrait};
use serde_json::Value;

use crate::lifecycle::entity::run_suspension;

use super::now;

pub async fn upsert_suspension(
    db: &DatabaseConnection,
    run_id: &str,
    prompt: &str,
    suggestions: &[String],
    resume_data: &SuspendedRunData,
) -> Result<(), DbErr> {
    let suggestions_val: Value = serde_json::to_value(suggestions).unwrap();
    let resume_val: Value = serde_json::to_value(resume_data).unwrap();
    let model = run_suspension::ActiveModel {
        run_id: Set(run_id.to_string()),
        prompt: Set(prompt.to_string()),
        suggestions: Set(suggestions_val),
        resume_data: Set(resume_val),
        created_at: Set(now()),
    };
    run_suspension::Entity::insert(model)
        .on_conflict(
            OnConflict::column(run_suspension::Column::RunId)
                .update_columns([
                    run_suspension::Column::Prompt,
                    run_suspension::Column::Suggestions,
                    run_suspension::Column::ResumeData,
                    // `created_at` means "when the CURRENT suspension began",
                    // not "when this run first suspended". One row per run, so
                    // without this an automation that delegates step 1, resumes,
                    // then delegates step 2 keeps step 1's timestamp — and
                    // [`get_suspension_with_start`], which the coordinator uses
                    // to survive a restart without resetting the suspend clock,
                    // would read step 5 as hours old and time it out at once.
                    run_suspension::Column::CreatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// The run's **current** suspension: when it began, and the checkpoint to
/// resume from.
///
/// Both in one read because `Coordinator::from_db` needs both for the same
/// task, and two `find_by_id` calls against the same row can disagree — one
/// fetch cannot.
///
/// The timestamp exists because the coordinator's suspend timeout is measured
/// from a `tokio::time::Instant`, which does not survive a process restart;
/// `from_db` would otherwise hand every recovered task a fresh full timeout.
/// This is the persisted, absolute counterpart, so a task already suspended for
/// three hours resumes with one hour left rather than four.
///
/// `None` means the run has no suspension row at all — never suspended, or
/// already resumed and cleaned up.
///
/// **The timestamp does not depend on the checkpoint parsing.** They are
/// returned as `(started, Option<data>)` rather than being collapsed into one
/// `Option`, because a `resume_data` that fails to deserialize must not take
/// `created_at` down with it. `SuspendedRunData` derives a plain `Deserialize`
/// with no field defaults, so adding a required field makes every pre-deploy
/// row unparseable — and recovery onto a new binary is exactly when
/// `Coordinator::from_db` runs. Such a task cannot resume either way, so it
/// should reach the suspend ceiling promptly; losing the timestamp would
/// instead hand it a fresh full timeout on every recovery, which under a deploy
/// cadence shorter than the ceiling means never.
pub async fn get_suspension_with_start(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<
    Option<(
        chrono::DateTime<chrono::FixedOffset>,
        Option<SuspendedRunData>,
    )>,
    DbErr,
> {
    Ok(run_suspension::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await?
        .map(|r| (r.created_at, parse_checkpoint(run_id, r.resume_data))))
}

/// Deserialize a stored checkpoint, saying so when it can't be read.
///
/// The `None` here is not a neutral "absent" — it means a suspended task can
/// never resume, and its only other symptom is a "delegation timed out" hours
/// later with nothing pointing at the cause. Swallowing the error silently is
/// what makes that a mystery instead of a one-line diagnosis; the most likely
/// producer is a `SuspendedRunData` shape change, which is a deploy event
/// somebody can act on.
fn parse_checkpoint(run_id: &str, resume_data: Value) -> Option<SuspendedRunData> {
    match serde_json::from_value(resume_data) {
        Ok(data) => Some(data),
        Err(e) => {
            tracing::warn!(
                target: "runtime",
                run_id,
                error = %e,
                // Deliberately stops at "cannot resume". Three consumers reach
                // here — coordinator, HTTP, pipeline — and what happens next
                // differs for each: the coordinator leaves a delegating parent
                // to its suspend ceiling, while the other two just see a run
                // with no checkpoint. Naming one consequence would be wrong for
                // the others. (Consumers, not call sites: the count of the
                // latter drifts.)
                "unparseable suspension checkpoint; this task cannot resume"
            );
            None
        }
    }
}

pub async fn get_suspension(
    db: &impl sea_orm::ConnectionTrait,
    run_id: &str,
) -> Result<Option<SuspendedRunData>, DbErr> {
    let row = run_suspension::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await?;
    Ok(row.and_then(|r| parse_checkpoint(run_id, r.resume_data)))
}
