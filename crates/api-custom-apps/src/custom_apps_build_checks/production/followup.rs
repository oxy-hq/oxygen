//! Runs the task that follows a failed production check ([`super::failed`])
//! on the worker fleet. Registered for [`PRODUCTION_FAILED_KIND`] by
//! `oxy-server`'s `custom_apps_seams`.
//!
//! In order ([`run`]): read the check's row, the move it checked and whether
//! the attempt had a baseline from before it ([`super::baseline`], read here
//! and never taken from the runner's report); roll production back when the
//! app says so and there was one ([`super::rollback`]); write what was found
//! and done on the row (`detail.move`, `detail.rollback`); tell people
//! ([`notify`]). The notice comes last because it says whether production
//! was rolled back.
//!
//! **Safe to run twice.** A row that already records its rollback is not
//! rolled back again; a task that died between its rollback and writing it
//! down finds the move in production's history
//! (`moved::platform_rollback_from`); and a person is told once per check
//! (the notice is deduplicated per recipient and subject).

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::Utc;
use entity::org_members::OrgRole;
use entity::{app_build_checks, app_builds, apps};
use futures::FutureExt;
use oxy_app_core::notifications::platform::{PlatformNotice, people};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::baseline;
use super::failed::{PRODUCTION_FAILED_KIND, ProductionFailedTask};
use super::moved::{self, Move};
use super::rollback;
use crate::custom_apps_build_checks::kind::{CheckKind, CheckStatus};
use crate::custom_apps_build_checks::settings::{self, Settings};
use crate::custom_apps_build_checks::store;

pub struct ProductionFailedExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for ProductionFailedExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let task = ProductionFailedTask::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let db = self.db.clone();
        tokio::spawn(async move {
            let _ = outcome_tx.send(run_guarded(&db, &task).await).await;
            drop(event_tx);
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

/// The task's outcome; a panic ends in `Failed` like any other failure.
pub async fn run_guarded(db: &DatabaseConnection, task: &ProductionFailedTask) -> TaskOutcome {
    let ran = std::panic::AssertUnwindSafe(run(db, task))
        .catch_unwind()
        .await;
    let metadata = json!({ "app_id": task.app_id, "build_pk": task.build_pk,
        "attempt": task.attempt });
    let why = match ran {
        Ok(Ok(answer)) => {
            return TaskOutcome::Done {
                answer,
                metadata: Some(metadata),
            };
        }
        Ok(Err(why)) => why,
        Err(_) => "the task panicked".to_string(),
    };
    tracing::error!(app_id = %task.app_id, build_pk = %task.build_pk,
        "{PRODUCTION_FAILED_KIND}: {why}");
    TaskOutcome::Failed(why)
}

/// The failed `production_smoke` row of `task`'s attempt; `None` when the
/// attempt has no such row or it did not fail.
async fn failed_check(
    db: &DatabaseConnection,
    task: &ProductionFailedTask,
) -> Result<Option<app_build_checks::Model>, sea_orm::DbErr> {
    Ok(store::rows_of(db, &task.key())
        .await?
        .into_iter()
        .find(|row| row.kind == CheckKind::ProductionSmoke.as_str())
        .filter(|row| row.status == CheckStatus::Failed.as_str()))
}

/// What the row is told about the move it checked. `baseline`: whether the
/// attempt had a baseline from before it that tells new errors apart; the
/// notice says "errors", not "new errors", where it did not.
async fn move_detail(
    db: &DatabaseConnection,
    checked: &Move,
    baselined: bool,
) -> Result<Value, sea_orm::DbErr> {
    Ok(
        json!({ "action": checked.action, "by": checked.by, "at": checked.at,
        "previous": moved::named(db, checked.previous).await?, "baseline": baselined }),
    )
}

/// What the rollback did, or `None` where the app does not roll back
/// automatically: what the row already records, else a rollback this task
/// made and could not record, else the rollback itself.
async fn rolled_back(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    check: &app_build_checks::Model,
    (history, checked): (&[moved::Event], &Move),
    baselined: bool,
) -> Result<Option<Value>, sea_orm::DbErr> {
    if !Settings::of(app).auto_rollback {
        return Ok(None);
    }
    if let Some(recorded) = rollback::recorded(check) {
        return Ok(Some(recorded));
    }
    if let Some(to) = moved::platform_rollback_from(history, build.id) {
        let to = moved::named(db, Some(to)).await?;
        return Ok(Some(
            json!({ "rolled_back": true, "to": to, "at": Utc::now().fixed_offset() }),
        ));
    }
    Ok(Some(
        rollback::roll_back(db, (app, build), check, checked, baselined).await,
    ))
}

/// Do what follows `task`'s failed check. `Ok` is the run's answer.
pub async fn run(db: &DatabaseConnection, task: &ProductionFailedTask) -> Result<String, String> {
    let db_err = |e: sea_orm::DbErr| format!("the database could not be read ({e})");
    let app = apps::Entity::find_by_id(task.app_id)
        .one(db)
        .await
        .map_err(db_err)?;
    let build = app_builds::Entity::find_by_id(task.build_pk)
        .one(db)
        .await
        .map_err(db_err)?;
    let (Some(app), Some(build)) = (app, build) else {
        return Ok("the app or the build is gone; nothing to follow".to_string());
    };
    if !settings::enabled_for(app.org_id) {
        return Ok("the promote checks gate is off for the app's org; nothing done".to_string());
    }
    let Some(check) = failed_check(db, task).await.map_err(db_err)? else {
        return Ok("the attempt has no failed production check; nothing done".to_string());
    };
    let history = moved::history(db, app.id).await.map_err(db_err)?;
    let Some(checked) = moved::move_in(&history, build.id) else {
        return Ok("production's history names no move to this build; nothing done".to_string());
    };
    follow(db, (&app, &build), &check, (&history, &checked))
        .await
        .map_err(db_err)
}

/// What follows `check`, the failed check of the move `checked`: whether
/// the attempt had a baseline, the rollback, the row's record of both, and
/// the notice. The run's answer.
async fn follow(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    check: &app_build_checks::Model,
    (history, checked): (&[moved::Event], &Move),
) -> Result<String, sea_orm::DbErr> {
    let baselined = baseline::tells_apart_for(db, build).await?;
    let rollback = rolled_back(db, (app, build), check, (history, checked), baselined).await?;
    let mut patch = json!({ "move": move_detail(db, checked, baselined).await? });
    if let Some(rollback) = &rollback {
        patch["rollback"] = rollback.clone();
    }
    store::merge_detail(db, check.id, patch).await?;
    let told = notify(db, app, check, checked.by).await?;
    let rolled = rollback
        .as_ref()
        .is_some_and(|r| r.get("rolled_back") == Some(&Value::Bool(true)));
    Ok(format!(
        "{told} told; production {}",
        if rolled {
            "rolled back"
        } else {
            "left as it is"
        }
    ))
}

/// Who a failed production check is for: whoever moved production, and the
/// org's owners and admins; active accounts only, each once.
pub async fn recipients(
    db: &DatabaseConnection,
    org_id: Uuid,
    moved_by: Option<Uuid>,
) -> Result<Vec<Uuid>, sea_orm::DbErr> {
    let mut who = people::active(db, moved_by.into_iter().collect()).await?;
    for officer in people::org_members_from(db, org_id, OrgRole::Admin).await? {
        if !who.contains(&officer) {
            who.push(officer);
        }
    }
    Ok(who)
}

/// Raise the notice for `check`. The number of people told now.
pub async fn notify(
    db: &DatabaseConnection,
    app: &apps::Model,
    check: &app_build_checks::Model,
    moved_by: Option<Uuid>,
) -> Result<usize, sea_orm::DbErr> {
    let notice = PlatformNotice::ProductionCheckFailed {
        org_id: app.org_id,
        workspace_id: app.project_id,
        check_id: check.id,
    };
    let who = recipients(db, app.org_id, moved_by).await?;
    let sent = oxy_app_core::notifications::platform::notify(db, &notice, &who).await?;
    Ok(sent.recipients)
}
