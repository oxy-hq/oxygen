//! Who queues a build's checks: every move that makes staging serve a build
//! (a publish to the channels, a sandbox agent token's draft, a sandbox's
//! build promoted to staging, staging rolled back), and a person asking again.
//!
//! **And production, after a move** ([`after_production_move`]): where the
//! app's org has the `promote_checks_gate` flag on and the app's
//! `production_smoke` setting is on, every move of production's pointer to
//! another build (a promote, Make Live, promote latest, a promoting publish, a
//! rollback, the automatic one included) queues a **production attempt** of
//! that build: one kind, `production_smoke`. Each move to a build queues that
//! build's next attempt, so a build promoted, rolled away from and promoted
//! again is checked each time. Flag off, or the setting off: nothing.
//!
//! **Never a failed move.** A move that cannot queue its checks answers with a
//! warning and stands; its build shows no checks until someone asks again.
//!
//! **A rerun waits only for an attempt that is still answering.** It is
//! refused while the newest attempt has a check open, unless that attempt is
//! stalled ([`super::stalled`]: no runner claimed it in time, or its runner
//! stopped) or nothing was started for it (a deployment with no runner
//! configured). Such an attempt is closed first, its open checks written as
//! they read (`error` when stalled, `skipped` when nothing was started), and
//! the next attempt is queued at once.
//!
//! **Boxed where a move awaits it.** The three a move calls
//! ([`after_staging_move`], [`after_staging_move_of`],
//! [`after_production_move`]) are plain `fn`s that hand back a boxed future,
//! so the mover's future holds a pointer. Awaited as `async fn`s, a debug
//! build reserves each one's whole future in the mover's poll frame, and the
//! promoting publish's frames (`custom_apps_publish::pipeline`) took a test
//! thread past its 2 MiB stack.

use chrono::Utc;
use entity::{app_builds, apps};
use futures::future::BoxFuture;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::environment::CheckEnvironment;
use super::kind::Outcome;
use super::settings::{self, Settings};
use super::stalled;
use super::store;
use super::task::{self, BuildChecksTask};
use crate::custom_apps_environments;
use crate::custom_apps_nonproduction::staging_task::StagingBuild;

fn first_attempt(build: &StagingBuild<'_>) -> BuildChecksTask {
    BuildChecksTask {
        app_id: build.app_id,
        app_slug: build.app_slug.to_string(),
        org_id: build.org_id,
        workspace_id: build.workspace_id,
        build_pk: build.build_pk,
        environment: AppEnvironment::Staging.name(),
        attempt: 1,
    }
}

/// Queue the first attempt of the checks of the build staging now serves.
/// The warnings are the move's: only a task that could not be queued has one.
/// A build checked before (staging rolled back to it) queues nothing.
pub fn after_staging_move<'a>(
    db: &'a DatabaseConnection,
    build: StagingBuild<'a>,
) -> BoxFuture<'a, Vec<String>> {
    Box::pin(queue_first_attempt(db, build))
}

async fn queue_first_attempt(db: &DatabaseConnection, build: StagingBuild<'_>) -> Vec<String> {
    let task = first_attempt(&build);
    match task::enqueue(db, &task).await {
        Ok(_) => Vec::new(),
        Err(e) => {
            tracing::warn!(app_id = %build.app_id, build_pk = %build.build_pk, error = %e,
                "could not queue the build's checks");
            vec![format!(
                "the build's checks were not queued ({e}); the move went on, and the checks can \
                 be run again from the console or `POST …/builds/{{build}}/checks/rerun`"
            )]
        }
    }
}

/// [`after_staging_move`] for a move that holds the app and build rows.
pub fn after_staging_move_of<'a>(
    db: &'a DatabaseConnection,
    app: &'a apps::Model,
    build: &'a app_builds::Model,
) -> BoxFuture<'a, Vec<String>> {
    let staged = StagingBuild {
        app_id: app.id,
        app_slug: &app.slug,
        workspace_id: app.project_id,
        org_id: app.org_id,
        build_pk: build.id,
    };
    after_staging_move(db, staged)
}

/// Queue a production attempt of `build`'s checks, now that production
/// serves it: `previous` is what production served before the move. Nothing
/// where the move re-stamped the same build, where the gate is off for the
/// app's org, or where the app does not check production. The warnings are
/// the move's: only a task that could not be queued has one.
pub fn after_production_move<'a>(
    db: &'a DatabaseConnection,
    app: &'a apps::Model,
    build: &'a app_builds::Model,
    previous: Option<Uuid>,
) -> BoxFuture<'a, Vec<String>> {
    Box::pin(queue_production_attempt(db, app, build, previous))
}

async fn queue_production_attempt(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
    previous: Option<Uuid>,
) -> Vec<String> {
    let checked = settings::enabled_for(app.org_id) && Settings::of(app).production_smoke;
    if previous == Some(build.id) || !checked {
        return Vec::new();
    }
    let production = CheckEnvironment::Production.name();
    let queued = async {
        let latest = store::latest_attempt(db, build.id, production).await?;
        let task = BuildChecksTask {
            app_id: app.id,
            app_slug: app.slug.clone(),
            org_id: app.org_id,
            workspace_id: app.project_id,
            build_pk: build.id,
            environment: production.to_string(),
            attempt: latest.unwrap_or(0) + 1,
        };
        task::enqueue(db, &task).await
    };
    match queued.await {
        Ok(_) => Vec::new(),
        Err(e) => {
            tracing::warn!(app_id = %app.id, build_pk = %build.id, error = %e,
                "could not queue production's check of the build");
            vec![format!(
                "production's check of build {:?} was not queued ({e}); the move went on",
                build.build_id
            )]
        }
    }
}

/// Why a rerun was not queued.
#[derive(Debug, thiserror::Error)]
pub enum RerunRefusal {
    #[error("staging does not serve build {build}; checks run on the build staging serves")]
    NotOnStaging { build: String },
    #[error("attempt {attempt} of this build's checks is still running")]
    Running { attempt: i32 },
    #[error("{0}")]
    Db(String),
}

impl RerunRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotOnStaging { .. } => "build_not_on_staging",
            Self::Running { .. } => "checks_running",
            Self::Db(_) => "internal",
        }
    }
}

/// Queue the next attempt of `build`'s checks on staging.
pub async fn rerun(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Result<BuildChecksTask, RerunRefusal> {
    let db_err = |e: sea_orm::DbErr| RerunRefusal::Db(e.to_string());
    let staging = AppEnvironment::Staging;
    let serving = custom_apps_environments::pointer(db, app.id, &staging)
        .await
        .map_err(db_err)?;
    if serving != Some(build.id) {
        return Err(RerunRefusal::NotOnStaging {
            build: build.build_id.clone(),
        });
    }
    let latest = store::latest_attempt(db, build.id, &staging.name())
        .await
        .map_err(db_err)?;
    if let Some(attempt) = latest
        && !settle_latest(db, build.id, attempt).await.map_err(db_err)?
    {
        return Err(RerunRefusal::Running { attempt });
    }
    let staged = StagingBuild {
        app_id: app.id,
        app_slug: &app.slug,
        workspace_id: app.project_id,
        org_id: app.org_id,
        build_pk: build.id,
    };
    let next = BuildChecksTask {
        attempt: latest.unwrap_or(0) + 1,
        ..first_attempt(&staged)
    };
    match task::enqueue(db, &next).await.map_err(db_err)? {
        true => Ok(next),
        // Another request queued this attempt between the read and the write.
        false => Err(RerunRefusal::Running {
            attempt: next.attempt,
        }),
    }
}

/// Whether `attempt` leaves room for the next one: `true` when every check
/// of it has a verdict, when it is stalled, and when nothing was started for
/// it; in the last two its open checks are closed here as they read (`error`,
/// or `skipped`), saying why. `false` while it still answers.
async fn settle_latest(
    db: &DatabaseConnection,
    build_pk: Uuid,
    attempt: i32,
) -> Result<bool, sea_orm::DbErr> {
    let key = store::AttemptKey {
        build_pk,
        environment: AppEnvironment::Staging.name(),
        attempt,
    };
    let rows = store::rows_of(db, &key).await?;
    if !rows.iter().any(stalled::is_waiting) {
        // An advisory check a dead runner left open is closed with its attempt.
        if let Some(why) = stalled::abandoned(&rows, Utc::now()) {
            store::close_open(db, &key, &Outcome::error(why.summary())).await?;
        }
        return Ok(true);
    }
    let closing = match stalled::not_started_of(&rows) {
        Some(marker) => {
            // The whole summary is kept: it ends with the command to run.
            let mut skipped = Outcome::skipped("");
            skipped.summary = marker.summary(rows[0].app_id, build_pk);
            skipped
        }
        None => match stalled::stalled(&rows, Utc::now()) {
            Some(why) => Outcome::error(why.summary()),
            None => return Ok(false),
        },
    };
    let closed = store::close_open(db, &key, &closing).await?;
    tracing::info!(%build_pk, attempt, closed, status = closing.status.as_str(),
        "build checks: an attempt nobody answers was closed");
    Ok(true)
}
