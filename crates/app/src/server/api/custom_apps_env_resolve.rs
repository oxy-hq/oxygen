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
//! **A sandbox resolves from its own row, uncached, with no fallback.** A
//! `dev-<handle>` environment (`internal-docs/custom-app-sandboxes.md`) is one
//! `app_environments` row of `kind = 'dev'`. [`resolve_environment`] and
//! [`resolve_function_environment`] read it directly ([`sandbox_row`]) rather
//! than through [`EnvironmentBuilds`], which stays the two fixed environments:
//! a sandbox with no build, no row, or a row being torn down serves nothing —
//! never staging's or production's build.
//!
//! **The semantic pin rides the build, unchanged.** A staging build may pin a
//! compiled semantic revision (`app_builds.semantic_revision_id`, the staging
//! semantic-model work in `internal-docs/customer-apps-staging.md`). Nothing here
//! copies or re-derives it: whatever resolves the staging environment gets
//! `staging`'s `build_id`, and the pin is read from that build
//! (`custom_apps_staging_pin::pinned_revision_for`) exactly as the draft preview
//! reads it — the staging row mirrors `draft_build_id`, so it is the same build.

use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder};
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

/// The builds an app's **fixed** environments serve, read once per app and
/// cached with the app's resolution. A sandbox is not in here: it resolves by
/// its own uncached row read ([`sandbox_row`]), so [`Self::resolve`] answers
/// nothing for one — ask [`resolve_environment`] instead.
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

    /// The non-production environment that serves `build` while production
    /// does not — for a reader deciding whether naming a build names a
    /// non-production environment. Both halves are
    /// [`Self::resolve_for_functions`]' answers, the ones the function runtime
    /// acts on, so the build a never-promoted app runs on the production path
    /// is production's. `None` when production serves `build`, or when no
    /// environment does (a retained build nothing points at any more).
    ///
    /// A sandbox is not in here (see the type docs), so this answers for
    /// staging alone: [`non_production_environment_serving`] is the whole
    /// rule, sandboxes included, and what a reader should ask.
    pub fn serves_only_outside_production(&self, build: Uuid) -> Option<AppEnvironment> {
        let serves = |environment: &AppEnvironment| {
            self.resolve_for_functions(environment).build_id == Some(build)
        };
        (!serves(&AppEnvironment::Production) && serves(&AppEnvironment::Staging))
            .then_some(AppEnvironment::Staging)
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
    if matches!(environment, AppEnvironment::Dev { .. }) {
        return resolve_sandbox(db, app.id, environment).await;
    }
    Ok(builds_for(db, app, environment).await?.resolve(environment))
}

/// [`resolve_environment`] for the function readers: `/fn`, the scheduled
/// runner, a queued job, and the manifest-derived function policy.
pub async fn resolve_function_environment<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<ResolvedEnvironment, DbErr> {
    if matches!(environment, AppEnvironment::Dev { .. }) {
        // No fallback: a sandbox with no build runs nothing, never staging's
        // or production's functions.
        return resolve_sandbox(db, app.id, environment).await;
    }
    Ok(builds_for(db, app, environment)
        .await?
        .resolve_for_functions(environment))
}

/// The row of the sandbox `environment` of `app_id`: `None` when there is no
/// such sandbox, when it is being torn down (`deleting_at` set — it serves
/// nothing from that moment), and for an environment that is not a sandbox.
///
/// One primary-key read, **uncached** on purpose
/// (`internal-docs/custom-app-sandboxes.md`): a publish to a sandbox and a
/// delete of one take effect at once on every replica, where the fixed
/// environments' builds ride the 60 s app-resolution cache. Only a request
/// that names a sandbox pays it; production and staging never reach here.
pub async fn sandbox_row<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<Option<app_environments::Model>, DbErr> {
    if !matches!(environment, AppEnvironment::Dev { .. }) {
        return Ok(None);
    }
    serving_sandboxes(app_id)
        .filter(app_environments::Column::Name.eq(environment.name()))
        .one(db)
        .await
}

/// `app_id`'s sandboxes that can serve: rows of `kind = 'dev'` not being torn
/// down. The one statement of it — [`sandbox_row`] narrows it to a name (the
/// primary key), [`non_production_environment_serving`] to builds.
fn serving_sandboxes(app_id: Uuid) -> sea_orm::Select<app_environments::Entity> {
    app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .filter(app_environments::Column::DeletingAt.is_null())
}

/// The non-production environment that serves one of `builds` while
/// production does not — for a reader deciding whether naming a build names
/// a non-production environment. Staging's answer is
/// [`EnvironmentBuilds::serves_only_outside_production`]; a sandbox's is its
/// own row, the one [`sandbox_row`] resolves it from, read for every sandbox
/// of the app in **one** query. `None` when production serves every named
/// build, or when no environment serves one (a retained build nothing points
/// at any more, a torn-down sandbox's included).
pub async fn non_production_environment_serving<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    builds: &[Uuid],
) -> Result<Option<AppEnvironment>, DbErr> {
    if builds.is_empty() {
        return Ok(None);
    }
    let fixed = load_environment_builds(db, app).await?;
    let staging = builds
        .iter()
        .find_map(|build| fixed.serves_only_outside_production(*build));
    if staging.is_some() {
        return Ok(staging);
    }
    let production = fixed
        .resolve_for_functions(&AppEnvironment::Production)
        .build_id;
    let outside = builds.iter().copied().filter(|b| Some(*b) != production);
    let sandbox = serving_sandboxes(app.id)
        .filter(app_environments::Column::BuildId.is_in(outside))
        .order_by_asc(app_environments::Column::Name)
        .one(db)
        .await?;
    Ok(sandbox.and_then(|row| AppEnvironment::parse(&row.name)))
}

/// A sandbox's own build, or nothing — never another environment's.
async fn resolve_sandbox<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<ResolvedEnvironment, DbErr> {
    let row = sandbox_row(db, app_id, environment).await?;
    Ok(ResolvedEnvironment {
        environment: environment.clone(),
        build_id: row.and_then(|row| row.build_id),
    })
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
