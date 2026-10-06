//! **The one place a function run is admitted or refused by its environment.**
//!
//! Every path that runs a function — a route call (`/fn`), a scheduled fire, a
//! queued job — resolves its environment first
//! (`custom_apps_env_resolve::resolve_function_environment`) and then asks
//! [`admit`]. Nothing else in the runtime decides by environment, and no host
//! call re-derives one: an [`Admission`] carries the environment's
//! [`EnvPolicy`], which the runtime hands to `ProjectFunctionHost` at
//! construction, and every host op asks that policy.
//!
//! **Production, and every non-production environment for Oxy staff.**
//! Production is admitted for every caller, with a policy that allows every op
//! — byte-identical to before environments. Staging and a sandbox
//! (`dev-<handle>`, `internal-docs/custom-app-sandboxes.md`) share one rule:
//! admitted only for a **route** call from a viewer who may open
//! non-production (`AppNonProduction`, the rule the staging host uses to serve
//! staging HTML), with that environment's policy, which isolates a write to
//! the environment's own home or holds it (`env_policy`): nothing a
//! non-production function does reaches production, which is invariant 2 held
//! by the host rather than by refusing the run. Such a run reads production
//! data, runs the environment's build, and sees `ctx.channel` as the
//! environment's name.
//!
//! Still refused: a non-production environment from a queued path (schedules,
//! webhooks and Run now stay production-only — their identities carry no
//! environment yet, design §4.2). A caller that has already decided the reach
//! for a queued run — a check run requested by staff — passes
//! `Entrance::Route { non_production_reach: true }` and carries that decision
//! with the task; this gate adds no entrance for it.
//!
//! What an environment's policy *does* lands in `env_policy`, not here. The
//! one fact it needs from the database is decided here, once per admitted
//! run: where its `ctx.oltp` lands ([`with_oltp_home`], P4b). The org has at
//! most one OLTP staging branch; staging runs in the app's schema there, and
//! a sandbox in a schema of its own, when its row records one ready
//! (`custom_apps_sandboxes::oltp_state`).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::env_policy::{EnvPolicy, OltpHome, SandboxUnready};
use crate::server::api::custom_apps_env_resolve::{ResolvedEnvironment, may_open_non_production};

/// How a run reached the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entrance {
    /// `/fn` with an authenticated viewer; `non_production_reach` is whether
    /// that viewer may open this app's non-production environments.
    Route { non_production_reach: bool },
    /// `/fn` with a **sandbox agent token** (`oxy_sbx_`). `own_sandbox` is
    /// whether the environment is a sandbox that token created, of an app it
    /// is granted (`agent_gate`). The token runs a function there and nowhere
    /// else — not in production, which every other caller is admitted to.
    SandboxAgent { own_sandbox: bool },
    /// A schedule, webhook, Airway step or manual job: no viewer.
    Queued,
}

/// A run the gate let through: where it runs, and what it may do there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Admission {
    pub environment: ResolvedEnvironment,
    pub policy: EnvPolicy,
}

/// Why a run was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefusedReason {
    /// Staging and sandboxes are Oxy staff's; this viewer is not.
    NotStaff,
    /// A non-production environment runs only on a staff route call.
    QueuedOutsideProduction,
    /// A sandbox agent token, anywhere but a sandbox it created.
    NotOwnSandbox,
}

/// A function run refused because of the environment it would run in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentRefused {
    pub environment: AppEnvironment,
    pub reason: RefusedReason,
}

impl EnvironmentRefused {
    pub(crate) fn message(&self) -> String {
        let env = &self.environment;
        match self.reason {
            RefusedReason::NotStaff => format!(
                "functions in the {env} environment run for Oxy staff only; call the function \
                 in production"
            ),
            RefusedReason::QueuedOutsideProduction => format!(
                "only a route call runs a function in the {env} environment; schedules, \
                 webhooks and manual runs are production-only"
            ),
            RefusedReason::NotOwnSandbox => {
                "a sandbox agent token runs functions only in a sandbox it created".to_string()
            }
        }
    }

    /// `403` with the reason — except to a sandbox agent token, which is
    /// answered as an unknown app is: `404 not_found` in the token's one body
    /// (`custom_apps_agent_body`), the same bytes, so it learns nothing of
    /// production, staging or another creator's sandbox.
    pub(crate) fn into_response(self) -> Response {
        if self.reason == RefusedReason::NotOwnSandbox {
            return crate::server::api::custom_apps_agent_body::Refusal::not_found()
                .into_response();
        }
        (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "EnvironmentRefused",
                "environment": self.environment.name(),
                "message": self.message(),
            })),
        )
            .into_response()
    }
}

/// May a function run in `resolved`, reached through `entrance`? See the
/// module docs for the rule.
pub(crate) fn admit(
    resolved: &ResolvedEnvironment,
    entrance: Entrance,
) -> Result<Admission, EnvironmentRefused> {
    let refused = |reason| EnvironmentRefused {
        environment: resolved.environment.clone(),
        reason,
    };
    use AppEnvironment::{Dev, Production, Staging};
    match (&resolved.environment, entrance) {
        // Stated first, so the arm that admits production to every caller
        // never sees this entrance. This is the second refusal of a
        // production call by a sandbox agent token (sandbox agent credential
        // design, decision 6): it reads the entrance and the environment and
        // nothing the route allow-list reads.
        (Dev { .. }, Entrance::SandboxAgent { own_sandbox: true }) => Ok(Admission {
            environment: resolved.clone(),
            policy: EnvPolicy::for_environment(resolved.environment.clone()),
        }),
        (_, Entrance::SandboxAgent { .. }) => Err(refused(RefusedReason::NotOwnSandbox)),
        (Production, _)
        | (
            Staging | Dev { .. },
            Entrance::Route {
                non_production_reach: true,
            },
        ) => Ok(Admission {
            environment: resolved.clone(),
            policy: EnvPolicy::for_environment(resolved.environment.clone()),
        }),
        (Staging | Dev { .. }, Entrance::Route { .. }) => Err(refused(RefusedReason::NotStaff)),
        (Staging | Dev { .. }, Entrance::Queued) => {
            Err(refused(RefusedReason::QueuedOutsideProduction))
        }
    }
}

/// `admission`, with where its `ctx.oltp` lands decided — once, here, for the
/// whole invocation (env design §4.2; `env_policy::OltpHome`). Production
/// always writes its own database and pays no lookup. Outside production, one
/// lookup of the org's staging branch ([`oltp_home_for`]); a sandbox in an
/// org that has one also reads its own row ([`sandbox_oltp_home`]).
pub(crate) async fn with_oltp_home(
    db: &sea_orm::DatabaseConnection,
    mut admission: Admission,
    app: &entity::apps::Model,
) -> Result<Admission, sea_orm::DbErr> {
    if admission.policy.is_production() {
        return Ok(admission);
    }
    let (_, branch) =
        oxy_oltp::branches::find(db, app.org_id, oxy_oltp::OltpBranch::Staging).await?;
    let home = match (oltp_home_for(branch.as_ref()), branch.as_ref()) {
        (OltpHome::StagingBranch(_), Some(row)) if is_sandbox(&admission) => {
            let cut = oxy_oltp::branches::BranchCut::of(row);
            sandbox_oltp_home(db, app, admission.policy.environment(), &cut).await?
        }
        (home, _) => home,
    };
    admission.policy = admission.policy.with_oltp_home(home);
    Ok(admission)
}

fn is_sandbox(admission: &Admission) -> bool {
    matches!(admission.policy.environment(), AppEnvironment::Dev { .. })
}

/// A sandbox's OLTP home in an org whose staging branch is cut as `cut`: its
/// own schema there when its row records one ready on that cut, and a refusal
/// that says why otherwise — never the schema staging uses.
async fn sandbox_oltp_home(
    db: &sea_orm::DatabaseConnection,
    app: &entity::apps::Model,
    environment: &AppEnvironment,
    cut: &oxy_oltp::branches::BranchCut,
) -> Result<OltpHome, sea_orm::DbErr> {
    use crate::server::api::custom_apps_sandboxes::oltp_state;
    let Some(schema) = sandbox_schema_of(&app.slug, environment) else {
        return Ok(OltpHome::SandboxUnready(SandboxUnready::NoSchemaName));
    };
    let state = oltp_state::read(db, app.id, environment).await?;
    Ok(oltp_state::home_for(state.as_ref(), &schema, cut))
}

/// The schema `environment` of the app `slug` has on the org's staging
/// branch, when the two name one: derived, as `ctx.oltp`'s writer is, from
/// the slug — never from a manifest or a request.
pub(crate) fn sandbox_schema_of(
    slug: &str,
    environment: &AppEnvironment,
) -> Option<oxy_oltp::sandbox_schema::SandboxSchema> {
    let label = match environment {
        AppEnvironment::Dev { .. } => environment.schema_label()?,
        _ => return None,
    };
    let writer = oxy_oltp::schema::app_writer_name(slug)
        .and_then(|name| oxy_oltp::WriterRef::app(name).ok())?;
    oxy_oltp::sandbox_schema::SandboxSchema::for_writer(&writer, &label).ok()
}

/// The home a staging run's `ctx.oltp` gets from the org's branch row: the
/// branch only when it is `active`. None, or one mid-reset or half-made,
/// keeps production's database read-only (Phase 3's hold) for this whole
/// invocation.
fn oltp_home_for(branch: Option<&oxy_oltp::entity::branches::Model>) -> OltpHome {
    match branch {
        Some(row) if row.status == oxy_oltp::entity::branches::BranchStatus::Active => {
            OltpHome::StagingBranch(row.provider_branch_id.clone())
        }
        _ => OltpHome::Production,
    }
}

/// The [`Entrance`] of a route call. Only a call outside production pays for
/// the reach decision (`may_open_non_production`, cached 60 s). A sandbox
/// agent token takes an entrance of its own (`agent_gate`).
pub(crate) async fn route_entrance(
    db: &sea_orm::DatabaseConnection,
    resolved: &ResolvedEnvironment,
    caller: &crate::server::authz::Caller,
    app: &entity::apps::Model,
) -> Entrance {
    if caller.is_sandbox_agent() {
        // Boxed: this is awaited inside the function route's future, whose
        // size every caller pays for; the token's branch adds a pointer to it.
        return Box::pin(super::agent_gate::entrance(db, resolved, caller, app)).await;
    }
    let non_production_reach =
        !resolved.is_production() && may_open_non_production(db, caller, app).await;
    Entrance::Route {
        non_production_reach,
    }
}

/// `policy`, reading the semantic model `build_id` pins (#3370,
/// `custom_apps_staging_pin`) when it is not production's. Production never
/// looks: a promoted build always reads `workspaces.current_revision_id`.
#[cfg(feature = "custom-app-functions")]
pub(crate) async fn with_build_pin(
    db: &sea_orm::DatabaseConnection,
    policy: EnvPolicy,
    build_id: Uuid,
) -> EnvPolicy {
    if policy.is_production() {
        return policy;
    }
    let pin = crate::server::api::custom_apps_staging_pin::pinned_revision_for(db, build_id).await;
    policy.with_semantic_pin(pin)
}

#[cfg(test)]
#[path = "environment_gate_tests.rs"]
mod tests;
