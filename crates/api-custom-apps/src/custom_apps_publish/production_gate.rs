//! A promoting publish meets production's blockers, the same table `oxyc
//! promote` meets (`custom_apps_promote::blockers`), computed by the same
//! functions from the same facts: the required keys production has no value
//! for (`custom_apps_secrets::required`), the registered schedules the build
//! retires (`custom_apps_promote::schedules`), its validation, and the app's
//! four-eyes rule (`custom_apps_promote::second_person`).
//!
//! **Where it runs, and what a refusal leaves.** After the build row and its
//! functions are recorded — the facts are read from them, as a promote's plan
//! reads them — and before the build's migrations are applied to production
//! or any pointer moves. A refusal is a `409` naming every blocker, and the
//! pipeline's one compensation (`OrphanGuard`) undoes the publish: the build
//! row, its bytes, and the app row of a brand-new app. **The staging half
//! does not land.** That is how the pipeline is built — staging and
//! production move in one transaction, and a promoting publish whose
//! migrations fail is undone the same way — and it keeps a refused request a
//! request that changed nothing, so CI re-runs the same command, build id
//! included, once the blocker is cleared. To land on staging regardless,
//! publish without `--promote` and promote with `oxyc promote`.
//!
//! Invariant 1 is not asked here: this publish records staging's event for
//! the build in the transaction that moves production, which is the evidence
//! `move_locked` then finds under the lock.
//!
//! **The four-eyes rule refuses every promoting publish while it is on**:
//! the identity promoting the build is, by construction, the one that just
//! published it.
//!
//! **So does the promote checks gate** (`custom_apps_promote::checks_gate`),
//! where the app's org has the `promote_checks_gate` flag on and the app
//! requires any check: the build this publish just made has no staging
//! attempt, so every required check is unmet (`checks_not_passed`). A
//! promoting publish carries no override; publish to staging, let the checks
//! run, then `oxyc promote`.

use entity::{app_builds, apps};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::{PublishError, PublishInput};
use crate::custom_apps_build_state::BuildState;
use crate::custom_apps_promote::blockers::{self, Act, ProductionFacts};
use crate::custom_apps_promote::checks_gate::{self, Checks};
use crate::custom_apps_promote::schedules;
use crate::custom_apps_promote::{Blocker, PromoteError};
use crate::custom_apps_secrets::required;

/// Refuse a promoting publish production would refuse as a promote. A draft
/// passes untouched.
pub(super) async fn check(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
    input: &PublishInput,
) -> Result<(), PublishError> {
    if !input.promote {
        return Ok(());
    }
    let (app, build) = load(db, app_id, build_pk).await?;
    let required_unset = required::unset_in_production(db, &app, &build)
        .await
        .map_err(PublishError::Db)?;
    let removals = schedules::diff(db, &app, &build)
        .await
        .map_err(unread)?
        .removed_functions();
    let checks = checks_gate::read(db, &app, &build).await.map_err(unread)?;
    let blockers = refused_by(Facts {
        app: &app,
        build: &build,
        required_unset: &required_unset,
        schedule_removals: &removals,
        confirmed_removals: &input.confirm_schedule_removals,
        checks: checks.as_ref(),
    });
    if blockers.is_empty() {
        Ok(())
    } else {
        Err(PublishError::ProductionBlocked(blockers))
    }
}

/// What the gate reads, once loaded.
struct Facts<'a> {
    app: &'a apps::Model,
    build: &'a app_builds::Model,
    required_unset: &'a [String],
    schedule_removals: &'a [String],
    confirmed_removals: &'a [String],
    /// Where the build stands against the checks the app requires; `None`
    /// where the promote checks gate is off.
    checks: Option<&'a Checks>,
}

/// The pure half: the blockers of a promote by the build's own publisher.
fn refused_by(facts: Facts<'_>) -> Vec<Blocker> {
    let (blockers, _) = blockers::evaluate(ProductionFacts {
        act: Act::Promote,
        build_label: &facts.build.build_id,
        // Recorded in the transaction that moves production (module docs).
        staging_served: true,
        validation: &BuildState::of(facts.build).validation,
        required_unset: facts.required_unset,
        schedule_removals: facts.schedule_removals,
        confirmed_removals: facts.confirmed_removals,
        second_person_required: facts.app.require_second_person_to_promote,
        // This request published the build it promotes.
        mover_published: true,
        checks: facts.checks,
        // A promoting publish carries no override.
        checks_overridden: false,
    });
    blockers
}

async fn load(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
) -> Result<(apps::Model, app_builds::Model), PublishError> {
    let db_err = |e: sea_orm::DbErr| PublishError::Db(e.to_string());
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| PublishError::Db(format!("app {app_id} vanished mid-publish")))?;
    let build = app_builds::Entity::find_by_id(build_pk)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| PublishError::Db(format!("build {build_pk} vanished mid-publish")))?;
    Ok((app, build))
}

/// A read the gate could not make, with what it was doing — not the bare
/// "internal error" a promote's refusal shows its caller.
fn unread(e: PromoteError) -> PublishError {
    match e {
        PromoteError::Internal(detail) => PublishError::Db(detail),
        other => PublishError::Db(other.to_string()),
    }
}

/// The sentence a refused promoting publish answers with.
pub(super) fn refused_message(blockers: &[Blocker]) -> String {
    let reasons: Vec<&str> = blockers.iter().map(|b| b.message.as_str()).collect();
    format!(
        "production refused this promoting publish, so nothing of it was published (staging \
         included): {}",
        reasons.join("; ")
    )
}

#[cfg(test)]
#[path = "production_gate_tests.rs"]
mod tests;
