//! What a build would change against the build production serves now: the
//! plan's own diff (functions, schedules, migrations), read with no request
//! behind it.
//!
//! [`super::plan`] builds the same three for a promote somebody asked for, and
//! this calls the same readers (`functions_diff::diff`, `schedules::diff`,
//! `pending_in_production`), so the two cannot describe a build differently.
//! The reader here is the advisory summary of a build's checks
//! (`custom_apps_build_checks::ai`), which must still say something when one
//! part cannot be read: each part is read on its own, and a part that could
//! not be read is named in `unread` and left `None`. The functions are diffed
//! against what production serves, so when that could not be read they are
//! unread too, never diffed against nothing.

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DatabaseConnection;

use super::error::PromoteError;
use super::functions_diff::{self, FunctionChanges};
use super::plan::{BuildRef, MigrationPlan};
use super::production::load_build;
use super::schedules::{self, ScheduleChanges};
use crate::custom_apps_environments;
use crate::custom_apps_migrations as migrations;

/// A build against what production serves now.
#[derive(Debug, Clone)]
pub struct ProductionDiff {
    /// What production serves now; `None`: nothing.
    pub current: Option<BuildRef>,
    pub functions: Option<FunctionChanges>,
    pub schedules: Option<ScheduleChanges>,
    /// The files production's ledger has not recorded: a move applies them
    /// before the pointer moves.
    pub migrations: Option<MigrationPlan>,
    /// The parts that could not be read, by name.
    pub unread: Vec<&'static str>,
}

/// The build production serves now.
async fn serving(
    db: &DatabaseConnection,
    app: &apps::Model,
) -> Result<Option<app_builds::Model>, PromoteError> {
    let production = AppEnvironment::Production;
    let pointer = custom_apps_environments::pointer(db, app.id, &production)
        .await
        .map_err(|e| PromoteError::internal("read production's pointer", e))?;
    match pointer {
        Some(build) => Ok(Some(load_build(db, app.id, build).await?)),
        None => Ok(None),
    }
}

/// The migration files of `build` production has not applied.
async fn pending(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Result<MigrationPlan, PromoteError> {
    let declared = migrations::declared_in_build(build, &app.slug).await?;
    let (oltp, airhouse) = migrations::pending_in_production(db, app.id, &declared).await?;
    Ok(MigrationPlan {
        when: "before_move",
        oltp,
        airhouse,
    })
}

/// What the functions are diffed against: production's build once it was
/// read (`Some(None)` when production serves nothing, so every function is
/// new), or `None` when it could not be read. Diffed against "serves nothing"
/// then, every function would read as added.
fn functions_against<T, E>(serving: &Result<Option<T>, E>) -> Option<Option<&T>> {
    serving.as_ref().ok().map(Option::as_ref)
}

/// `build` of `app` against what production serves now.
pub async fn read(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
) -> ProductionDiff {
    let mut unread = Vec::new();
    let mut part = |name: &'static str, e: &dyn std::fmt::Display| {
        tracing::warn!(app_id = %app.id, build = %build.build_id, part = name, error = %e,
            "production diff: a part could not be read");
        unread.push(name);
    };
    let serving = serving(db, app)
        .await
        .map_err(|e| part("what production serves", &e));
    let functions = match functions_against(&serving) {
        Some(current) => functions_diff::diff(db, build, current)
            .await
            .map_err(|e| part("functions", &e))
            .ok()
            .map(|(changes, _warnings)| changes),
        None => {
            part("functions", &"what production serves could not be read");
            None
        }
    };
    let schedules = schedules::diff(db, app, build)
        .await
        .map_err(|e| part("schedules", &e))
        .ok();
    let migrations = pending(db, app, build)
        .await
        .map_err(|e| part("migrations", &e))
        .ok();
    ProductionDiff {
        current: serving.ok().flatten().map(|serving| BuildRef {
            id: serving.id,
            build_id: serving.build_id,
        }),
        functions,
        schedules,
        migrations,
        unread,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Production read: its build, or nothing when it serves nothing (every
    /// function is then new). Production not read: nothing to diff against,
    /// never "serves nothing".
    #[test]
    fn the_functions_are_diffed_only_against_a_production_that_was_read() {
        let build = 7_u8;
        assert_eq!(
            functions_against::<u8, ()>(&Ok(Some(build))),
            Some(Some(&build))
        );
        assert_eq!(functions_against::<u8, ()>(&Ok(None)), Some(None));
        assert_eq!(functions_against::<u8, ()>(&Err(())), None);
    }
}
