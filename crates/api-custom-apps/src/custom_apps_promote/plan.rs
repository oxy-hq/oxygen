//! The plan a promote or a rollback shows before it acts, and answers after
//! (environments design §5.3; §5.5a ruling 8, as built):
//!
//! - the build — id, publish id, commit, branch, repo, publisher, created,
//!   `published_to`;
//! - what the moving environment serves now (`current`, and `expect_current`
//!   for the act to send back);
//! - since when the source has served the build (a rollback: since when the
//!   environment last did);
//! - functions added, removed and changed;
//! - production only: schedule changes against the schedules registered now,
//!   `required` keys production has no value for, and whether the mover
//!   published the build (the app's four-eyes rule, `second_person`);
//! - migrations: for production the pending files per store, applied before
//!   the move; for staging the declared files, queued after it;
//! - production only, where the org's `promote_checks_gate` flag is on and the
//!   move is a promote: `checks`, where the build's newest staging attempt
//!   stands against the checks the app requires (`checks_gate`), whether the
//!   caller may override them, and the override the request carried;
//! - blockers and warnings.

use chrono::Utc;
use entity::{app_builds, users};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Serialize;
use uuid::Uuid;

use super::blockers::{self, Act, ProductionFacts};
use super::checks_gate::{self, Checks, Overridden};
use super::checks_override;
use super::error::{Blocker, EVENT_LISTENERS_BROKEN, PromoteError};
use super::functions_diff::{self, FunctionChanges};
use super::production::{MigrationsApplied, load_build};
use super::resolve::Resolved;
use super::schedules::{self, ScheduleChanges};
use super::second_person::Identity;
use crate::app_events::gate::{self, GateScope};
use crate::custom_apps_build_history as history;
use crate::custom_apps_build_state::BuildState;
use crate::custom_apps_migrations::{self as migrations, StoredMigrations, files_not_in};
use crate::custom_apps_secrets::required;

#[derive(Debug, Clone, Serialize)]
pub struct BuildSummary {
    /// `app_builds.id`.
    pub id: Uuid,
    /// The publish id (`--build-id`).
    pub build_id: String,
    pub commit_sha: Option<String>,
    pub source_branch: Option<String>,
    pub source_repo: Option<String>,
    pub published_by: Option<String>,
    pub published_via: Option<String>,
    pub created_at: String,
    pub published_to: Option<String>,
    pub validation_status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildRef {
    pub id: Uuid,
    pub build_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationPlan {
    /// `before_move` (production: applied, then the pointer moves) or
    /// `queued_after` (staging: queued once the pointer has moved).
    pub when: &'static str,
    pub oltp: Vec<String>,
    pub airhouse: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub action: &'static str,
    pub app_id: Uuid,
    pub from: Option<String>,
    pub to: String,
    pub build: BuildSummary,
    pub current: Option<BuildRef>,
    /// What `to` serves now, as the act sends it back (`null`: nothing).
    pub expect_current: Option<Uuid>,
    pub served_since: Option<String>,
    pub served_for_seconds: Option<i64>,
    pub functions: FunctionChanges,
    /// Production only.
    pub schedules: Option<ScheduleChanges>,
    pub migrations: MigrationPlan,
    /// Production only.
    pub required_secrets_unset: Vec<String>,
    pub blockers: Vec<Blocker>,
    pub warnings: Vec<String>,
    /// Production listeners this move breaks and `break_listeners` let
    /// through: the act audits them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub broken_listeners: Vec<String>,
    pub dry_run: bool,
    pub moved: bool,
    pub migrations_applied: Option<MigrationsApplied>,
    /// Where the build stands against the checks the app requires
    /// (`checks_gate`). `null` where the org's `promote_checks_gate` flag is
    /// off, for a rollback, and for a move of staging.
    pub checks: Option<Checks>,
}

/// What a request asks of the plan, beside what it names.
#[derive(Debug, Clone, Copy)]
pub struct Asked<'a> {
    /// The request's `confirm_schedule_removals`.
    pub confirmed: &'a [String],
    /// Who would move the environment (`second_person`).
    pub mover: &'a Identity,
    /// `Some(flag)` for a dry run, whose plan previews the event gate with the
    /// request's `break_listeners`; a real act passes `None`, and the
    /// production move is then the only place the gate runs.
    pub break_listeners: Option<bool>,
    /// What the request asks of the promote checks gate, and whether its
    /// caller may override (`checks_override::ask`).
    pub checks: &'a checks_override::GateAsk,
}

/// Everything the plan reads, for `r`.
pub async fn build(
    db: &DatabaseConnection,
    r: &Resolved,
    asked: Asked<'_>,
) -> Result<Plan, PromoteError> {
    let current = match r.current {
        Some(current) => Some(load_build(db, r.app.id, current).await?),
        None => None,
    };
    let (functions, mut warnings) = functions_diff::diff(db, &r.build, current.as_ref()).await?;
    let declared = migrations::declared_in_build(&r.build, &r.app.slug).await?;
    let migrations = plan_migrations(db, r, &declared).await?;
    if r.act == Act::Rollback {
        warnings.extend(predates_warning(r, current.as_ref(), &declared).await);
    }
    let (served_since, served_for_seconds) = served(db, r).await?;
    let mut plan = Plan {
        action: r.act.as_str(),
        app_id: r.app.id,
        from: r.from.as_ref().map(AppEnvironment::name),
        to: r.to.name(),
        build: summary(db, &r.build).await,
        current: current.map(|b| BuildRef {
            id: b.id,
            build_id: b.build_id,
        }),
        expect_current: r.current,
        served_since,
        served_for_seconds,
        functions,
        schedules: None,
        migrations,
        required_secrets_unset: Vec::new(),
        blockers: Vec::new(),
        warnings,
        broken_listeners: Vec::new(),
        dry_run: true,
        moved: false,
        migrations_applied: None,
        checks: None,
    };
    if r.to == AppEnvironment::Production {
        production_checks(db, r, asked, &mut plan).await?;
    }
    Ok(plan)
}

/// Production's half: schedules, required keys, who promotes, the checks
/// the app requires, the blockers, and a dry run's preview of the event gate.
async fn production_checks(
    db: &DatabaseConnection,
    r: &Resolved,
    asked: Asked<'_>,
    plan: &mut Plan,
) -> Result<(), PromoteError> {
    let changes = schedules::diff(db, &r.app, &r.build).await?;
    let required_unset = required::unset_in_production(db, &r.app, &r.build)
        .await
        .map_err(|e| PromoteError::internal("read production's secrets", e))?;
    let staging_served = history::has_served(db, r.app.id, &AppEnvironment::Staging, r.build.id)
        .await
        .map_err(|e| PromoteError::internal("read staging's history", e))?;
    let removals = changes.removed_functions();
    let mut checks = promote_checks(db, r).await?;
    let overridden = asked.checks.ask.accepted();
    let (blockers, warnings) = blockers::evaluate(ProductionFacts {
        act: r.act,
        build_label: &r.build.build_id,
        staging_served,
        validation: &BuildState::of(&r.build).validation,
        required_unset: &required_unset,
        schedule_removals: &removals,
        confirmed_removals: asked.confirmed,
        second_person_required: r.app.require_second_person_to_promote,
        mover_published: Identity::published(&r.build).is(asked.mover),
        checks: checks.as_ref(),
        checks_overridden: overridden.is_some(),
    });
    if let Some(checks) = checks.as_mut() {
        checks.overridable = asked.checks.may_override;
        checks.overridden = overridden.map(|accepted| Overridden {
            reason: accepted.reason().to_string(),
        });
    }
    plan.checks = checks;
    plan.schedules = Some(changes);
    plan.required_secrets_unset = required_unset;
    plan.blockers = blockers;
    plan.warnings.extend(warnings);
    event_listeners(db, r, asked.break_listeners, plan).await
}

/// Where a promote's build stands against the checks the app requires; `None`
/// for a rollback, the emergency path, and where the gate is off.
async fn promote_checks(
    db: &DatabaseConnection,
    r: &Resolved,
) -> Result<Option<Checks>, PromoteError> {
    match r.act {
        Act::Promote => checks_gate::read(db, &r.app, &r.build).await,
        Act::Rollback => Ok(None),
    }
}

/// A dry run's preview of the event gate (`app_events::gate`): a blocker per
/// listener the move would break, unless `break_listeners` lets them through.
/// A rollback only ever warns. Never run for a real act: the move enforces.
async fn event_listeners(
    db: &DatabaseConnection,
    r: &Resolved,
    break_listeners: Option<bool>,
    plan: &mut Plan,
) -> Result<(), PromoteError> {
    let Some(break_listeners) = break_listeners else {
        return Ok(());
    };
    let scope = match r.act {
        Act::Promote => GateScope::Ship { break_listeners },
        Act::Rollback => GateScope::Rollback,
    };
    let report = gate::check(db, &r.app, &r.build, scope)
        .await
        .map_err(|e| PromoteError::internal("check the event listeners", e))?;
    plan.warnings.extend(report.warnings);
    if break_listeners {
        plan.warnings.extend(report.refusals.iter().cloned());
        plan.broken_listeners = report.refusals;
    } else {
        plan.blockers
            .extend(report.refusals.into_iter().map(|message| Blocker {
                code: EVENT_LISTENERS_BROKEN,
                message,
            }));
    }
    Ok(())
}

/// Production: the files its ledger has not recorded, applied before the
/// move. Staging: every declared file, queued after it.
async fn plan_migrations(
    db: &DatabaseConnection,
    r: &Resolved,
    declared: &StoredMigrations,
) -> Result<MigrationPlan, PromoteError> {
    if r.to == AppEnvironment::Production {
        let (oltp, airhouse) = migrations::pending_in_production(db, r.app.id, declared).await?;
        return Ok(MigrationPlan {
            when: "before_move",
            oltp,
            airhouse,
        });
    }
    let (oltp, airhouse) = declared.filenames();
    Ok(MigrationPlan {
        when: "queued_after",
        oltp,
        airhouse,
    })
}

/// A rollback whose target carries fewer files than the build it replaces:
/// those tables stay — nothing is reversed — and the plan says so.
async fn predates_warning(
    r: &Resolved,
    current: Option<&app_builds::Model>,
    target: &StoredMigrations,
) -> Option<String> {
    let current = current?;
    let declared = match migrations::declared_in_build(current, &r.app.slug).await {
        Ok(declared) => declared,
        Err(e) => {
            return Some(format!(
                "could not read the current build's migrations to compare ({e})"
            ));
        }
    };
    let n = files_not_in(&declared, target);
    (n > 0).then(|| {
        format!(
            "the target predates {n} migration file(s) the current build carries; rollback never \
             reverses a migration, so those tables and columns stay"
        )
    })
}

/// Since when the source served the build (a rollback: the environment
/// itself), and for how long.
async fn served(
    db: &DatabaseConnection,
    r: &Resolved,
) -> Result<(Option<String>, Option<i64>), PromoteError> {
    let source = r.from.as_ref().unwrap_or(&r.to);
    let since = history::served_since(db, r.app.id, source, r.build.id)
        .await
        .map_err(|e| PromoteError::internal("read the environment's history", e))?;
    Ok(match since {
        Some(at) => (
            Some(at.to_rfc3339()),
            Some((Utc::now().fixed_offset() - at).num_seconds().max(0)),
        ),
        None => (None, None),
    })
}

/// The build as the plan shows it; the publisher's address when the user
/// still exists.
async fn summary(db: &DatabaseConnection, build: &app_builds::Model) -> BuildSummary {
    let published_by = match build.published_by {
        Some(user) => users::Entity::find_by_id(user)
            .one(db)
            .await
            .ok()
            .flatten()
            .map(|u| u.label().to_string()),
        None => None,
    };
    BuildSummary {
        id: build.id,
        build_id: build.build_id.clone(),
        commit_sha: build.commit_sha.clone(),
        source_branch: build.source_branch.clone(),
        source_repo: build.source_repo.clone(),
        published_by,
        published_via: build.published_via.clone(),
        created_at: build.created_at.to_rfc3339(),
        published_to: build.published_to.clone(),
        validation_status: build.validation_status.clone(),
    }
}
