//! Which build an app environment serves, and who may open one that is not
//! production — the read side of `app_environments`
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3, Phase 1b).
//!
//! Phase 1a mirrored every pointer write into `app_environments`; Phase 1b
//! switches the readers here, so every reader asks one question —
//! [`resolve_environment`] — instead of each picking `apps.published_build_id`
//! or `apps.draft_build_id` for itself. Phase 1c drops the columns once nothing
//! reads them.
//!
//! **A missing row falls back to its column.** 1a's backfill and mirrored writes
//! should leave no app with a pointer and no row; if one exists anyway, serving
//! what the column says keeps production exactly as it was, and the warning is
//! the evidence to go and find the writer that skipped the mirror.
//!
//! **Production asks no query.** [`resolve_environment`] and
//! [`resolve_function_environment`] answer production from the `apps` row the
//! caller already holds — `published_build_id`, exactly as every production
//! reader did before `app_environments` existed — and read `app_environments`
//! only for a non-production environment. Every production `/fn` call and
//! scheduled run goes through them, so a query there was a round trip on the
//! hottest path and turned a database blip into a 500 for production traffic
//! that never needed the table (PR #3381 review).
//!
//! **The functions fallback is today's, kept on purpose.** Every function
//! reader resolved `published_build_id.or(draft_build_id)`: an app that has
//! never been promoted runs its draft build's functions on the production path
//! (a staff draft preview of a new app depends on it).
//! [`EnvironmentBuilds::resolve_for_functions`] keeps exactly that for
//! production; removing it is a production behaviour change that belongs to the
//! promote path (Phase 3), not to this switch.
//!
//! **The semantic pin rides the build, unchanged.** A staging build may pin a
//! compiled semantic revision (`app_builds.semantic_revision_id`, the staging
//! semantic-model work in `internal-docs/customer-apps-staging.md`). Nothing here
//! copies or re-derives it: whatever resolves the staging environment gets
//! `staging`'s `build_id`, and the pin is read from that build
//! (`custom_apps_staging_pin::pinned_revision_for`) exactly as the draft preview
//! reads it — the staging row mirrors `draft_build_id`, so it is the same build.

use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

/// One app environment and the build it serves right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedEnvironment {
    pub environment: AppEnvironment,
    /// `None`: the environment serves nothing (no row, or a row with no build).
    pub build_id: Option<Uuid>,
}

impl ResolvedEnvironment {
    pub fn is_production(&self) -> bool {
        self.environment == AppEnvironment::Production
    }
}

/// The builds an app's fixed environments serve, read once per app. Dev slots
/// have no rows until the dev-slot API exists (Phase 4), so they resolve to
/// nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvironmentBuilds {
    pub production: Option<Uuid>,
    pub staging: Option<Uuid>,
}

impl EnvironmentBuilds {
    /// The build `environment` itself serves.
    pub fn resolve(&self, environment: &AppEnvironment) -> ResolvedEnvironment {
        let build_id = match environment {
            AppEnvironment::Production => self.production,
            AppEnvironment::Staging => self.staging,
            AppEnvironment::Dev { .. } => None,
        };
        ResolvedEnvironment {
            environment: environment.clone(),
            build_id,
        }
    }

    /// [`Self::resolve`], except that production falls back to staging's build
    /// for an app never promoted — what the function readers have always done
    /// (see the module docs).
    pub fn resolve_for_functions(&self, environment: &AppEnvironment) -> ResolvedEnvironment {
        let mut resolved = self.resolve(environment);
        if resolved.is_production() && resolved.build_id.is_none() {
            resolved.build_id = self.staging;
        }
        resolved
    }
}

/// Read `app`'s environment rows. A fixed environment with no row takes its
/// legacy column instead, with a warning (see the module docs).
pub async fn load_environment_builds<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
) -> Result<EnvironmentBuilds, DbErr> {
    let rows = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app.id))
        .filter(app_environments::Column::Name.is_in(["production", "staging"]))
        .all(db)
        .await?;
    let row = |name: &str| rows.iter().find(|r| r.name == name).map(|r| r.build_id);
    Ok(EnvironmentBuilds {
        production: from_row_or_column(
            app.id,
            "production",
            row("production"),
            app.published_build_id,
        ),
        staging: from_row_or_column(app.id, "staging", row("staging"), app.draft_build_id),
    })
}

/// [`load_environment_builds`] for many apps in **one** query — for list
/// endpoints, which must not pay a query per row.
pub async fn load_environment_builds_batch<C: ConnectionTrait>(
    db: &C,
    apps: &[apps::Model],
) -> Result<std::collections::HashMap<Uuid, EnvironmentBuilds>, DbErr> {
    let rows = if apps.is_empty() {
        Vec::new()
    } else {
        app_environments::Entity::find()
            .filter(app_environments::Column::AppId.is_in(apps.iter().map(|a| a.id)))
            .filter(app_environments::Column::Name.is_in(["production", "staging"]))
            .all(db)
            .await?
    };
    Ok(apps
        .iter()
        .map(|app| {
            let row = |name: &str| {
                rows.iter()
                    .find(|r| r.app_id == app.id && r.name == name)
                    .map(|r| r.build_id)
            };
            let builds = EnvironmentBuilds {
                production: from_row_or_column(
                    app.id,
                    "production",
                    row("production"),
                    app.published_build_id,
                ),
                staging: from_row_or_column(app.id, "staging", row("staging"), app.draft_build_id),
            };
            (app.id, builds)
        })
        .collect())
}

fn from_row_or_column(
    app_id: Uuid,
    environment: &str,
    row: Option<Option<Uuid>>,
    column: Option<Uuid>,
) -> Option<Uuid> {
    match row {
        Some(build) => build,
        None => {
            if column.is_some() {
                tracing::warn!(
                    %app_id,
                    environment,
                    "app has a build pointer but no app_environments row; serving the pointer"
                );
            }
            column
        }
    }
}

/// The builds as the `apps` row in hand says — production's answer, read
/// without a query (see the module docs).
fn from_app_row(app: &apps::Model) -> EnvironmentBuilds {
    EnvironmentBuilds {
        production: app.published_build_id,
        staging: app.draft_build_id,
    }
}

/// The builds to resolve `environment` from: the row in hand for production,
/// `app_environments` for anything else.
async fn builds_for<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<EnvironmentBuilds, DbErr> {
    match environment {
        AppEnvironment::Production => Ok(from_app_row(app)),
        _ => load_environment_builds(db, app).await,
    }
}

/// The build `environment` of `app` serves. Every reader outside the function
/// runtime asks this.
pub async fn resolve_environment<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<ResolvedEnvironment, DbErr> {
    Ok(builds_for(db, app, environment).await?.resolve(environment))
}

/// [`resolve_environment`] for the function readers: `/fn`, the scheduled
/// runner, a queued job, and the manifest-derived function policy.
pub async fn resolve_function_environment<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<ResolvedEnvironment, DbErr> {
    Ok(builds_for(db, app, environment)
        .await?
        .resolve_for_functions(environment))
}

/// Decisions of [`may_open_non_production`], per `(user_id, app_id)`, for the
/// same 60 s as every other step of the serve chain: a staging page load is the
/// same 30-100 asset requests as a production one, and the decision loads the
/// principal's facts. A revoked grant stops opening staging within a minute,
/// like a revoked membership stops opening production.
type DecisionCache =
    std::sync::RwLock<std::collections::HashMap<(Uuid, Uuid), (bool, std::time::Instant)>>;

fn non_production_cache() -> &'static DecisionCache {
    static CACHE: std::sync::OnceLock<DecisionCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// May this viewer open a **non-production** environment of `app`? Oxy staff
/// holding `develop_apps` over the app's org, decided by oxy-authz
/// (`Action::AppNonProduction`) with today's staff draft-preview gate as the
/// `existing_allow`, so the model can only narrow it. Cached 60 s.
pub async fn may_open_non_production(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    email: &str,
    app: &apps::Model,
) -> bool {
    use super::custom_apps_cache::{get_fresh, insert_with_sweep};
    let key = (user_id, app.id);
    if let Some(allowed) = get_fresh(non_production_cache(), &key) {
        return allowed;
    }
    let allowed = decide_non_production(db, user_id, email, app).await;
    insert_with_sweep(non_production_cache(), key, allowed);
    allowed
}

async fn decide_non_production(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    email: &str,
    app: &apps::Model,
) -> bool {
    let existing_allow = crate::server::authz::globals::platform_reaches(
        db,
        email,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await;
    crate::server::authz::enforce_for(
        db,
        user_id,
        email,
        "custom_app_non_production",
        oxy_authz::Action::AppNonProduction,
        oxy_authz::Resource::app(app.id, app.org_id),
        existing_allow,
    )
    .await
}

#[cfg(test)]
#[path = "custom_apps_env_resolve_tests.rs"]
mod tests;
