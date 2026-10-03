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
        }
    }

    pub(crate) fn into_response(self) -> Response {
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
/// the reach decision (`may_open_non_production`, cached 60 s).
pub(crate) async fn route_entrance(
    db: &sea_orm::DatabaseConnection,
    resolved: &ResolvedEnvironment,
    user_id: Uuid,
    email: Option<&str>,
    app: &entity::apps::Model,
) -> Entrance {
    let non_production_reach = !resolved.is_production()
        && may_open_non_production(db, user_id, email.unwrap_or(""), app).await;
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
mod tests {
    use super::*;
    use crate::server::api::custom_apps_functions::env_policy::{Decision, HostOp};

    fn resolved(environment: AppEnvironment) -> ResolvedEnvironment {
        ResolvedEnvironment {
            environment,
            build_id: Some(Uuid::nil()),
        }
    }

    const STAFF: Entrance = Entrance::Route {
        non_production_reach: true,
    };
    const VIEWER: Entrance = Entrance::Route {
        non_production_reach: false,
    };

    #[test]
    fn production_runs_for_everyone_and_allows_every_op() {
        for entrance in [STAFF, VIEWER, Entrance::Queued] {
            let admission = admit(&resolved(AppEnvironment::Production), entrance)
                .expect("production is always admitted");
            assert!(admission.policy.is_production());
            assert_eq!(admission.policy.decide(HostOp::StoragePut), Decision::Allow);
        }
    }

    /// Staging runs for staff on a route call, with a policy that holds its
    /// writes — the admission is what the host is built from.
    #[test]
    fn staging_runs_for_staff_with_writes_held() {
        let admission = admit(&resolved(AppEnvironment::Staging), STAFF).expect("staff");
        assert_eq!(admission.environment.environment, AppEnvironment::Staging);
        assert_eq!(
            admission.policy.decide(HostOp::OltpExec),
            Decision::Hold,
            "a staging write is held"
        );
        assert_eq!(admission.policy.decide(HostOp::Query), Decision::Allow);
    }

    #[test]
    fn staging_is_refused_to_a_viewer_who_is_not_staff_and_to_the_queue() {
        let env = resolved(AppEnvironment::Staging);
        let refused = admit(&env, VIEWER).expect_err("not staff");
        assert_eq!(refused.reason, RefusedReason::NotStaff);
        let refused = admit(&env, Entrance::Queued).expect_err("queued");
        assert_eq!(refused.reason, RefusedReason::QueuedOutsideProduction);
        assert!(
            refused.message().contains("staging"),
            "{}",
            refused.message()
        );
        assert_eq!(refused.into_response().status(), StatusCode::FORBIDDEN);
    }

    /// Only an `active` branch becomes staging's OLTP home; anything else
    /// holds on production.
    #[test]
    fn only_an_active_branch_is_the_oltp_home() {
        use oxy_oltp::entity::branches::{BranchStatus, Model};
        let row = |status| Model {
            id: Uuid::nil(),
            tenant_row_id: Uuid::nil(),
            kind: oxy_oltp::OltpBranch::Staging,
            provider_branch_id: "br-staging".into(),
            parent_branch_id: "br-main".into(),
            host: "h".into(),
            database_name: "d".into(),
            owner_role: "o".into(),
            owner_password_ciphertext: None,
            status,
            created_at: chrono::Utc::now().into(),
            last_reset_at: None,
            updated_at: chrono::Utc::now().into(),
        };
        assert_eq!(
            oltp_home_for(Some(&row(BranchStatus::Active))),
            OltpHome::StagingBranch("br-staging".into())
        );
        for status in [BranchStatus::Resetting, BranchStatus::Provisioning] {
            assert_eq!(oltp_home_for(Some(&row(status))), OltpHome::Production);
        }
        assert_eq!(oltp_home_for(None), OltpHome::Production);
    }

    fn sandbox() -> AppEnvironment {
        AppEnvironment::Dev {
            handle: "luong".into(),
        }
    }

    /// A sandbox's schema is the app's writer schema and the sandbox's label —
    /// the Airhouse sibling's name — and only a sandbox has one. A name over
    /// 63 bytes is none, not a shorter one.
    #[test]
    fn a_sandboxs_oltp_schema_is_derived_from_the_slug_and_the_sandbox() {
        let schema = sandbox_schema_of("store-ops", &sandbox()).expect("a schema");
        assert_eq!(schema.name(), "app_store_ops__dev_luong");
        assert_eq!(
            Some(schema.name().to_string()),
            airhouse::app_schema::environment_schema(
                "app_store_ops",
                &sandbox().schema_label().expect("a label")
            ),
            "one name in both stores"
        );
        assert!(sandbox_schema_of("store-ops", &AppEnvironment::Staging).is_none());
        assert!(sandbox_schema_of("store-ops", &AppEnvironment::Production).is_none());
        let long = "a".repeat(50);
        assert!(sandbox_schema_of(&long, &sandbox()).is_none(), "64+ bytes");
        assert!(sandbox_schema_of("store_ops", &sandbox()).is_none());
    }

    /// A sandbox is admitted on the arm staging is: a route call from staff,
    /// with the non-production policy of **that** environment — the host then
    /// isolates its writes to the sandbox's own homes or holds them.
    #[test]
    fn a_sandbox_runs_for_staff_with_the_non_production_policy() {
        let admission = admit(&resolved(sandbox()), STAFF).expect("staff");
        assert_eq!(admission.environment.environment, sandbox());
        assert_eq!(admission.policy.environment(), &sandbox());
        assert!(!admission.policy.is_production());
        assert_eq!(
            admission.policy.decide(HostOp::OltpExec),
            Decision::Hold,
            "a sandbox write with no isolated home is held"
        );
        assert_eq!(admission.policy.decide(HostOp::Query), Decision::Allow);
        let staging = admit(&resolved(AppEnvironment::Staging), STAFF).expect("staff");
        for op in HostOp::ALL {
            assert_eq!(
                admission.policy.decide(*op),
                staging.policy.decide(*op),
                "{op:?}: a sandbox decides as staging"
            );
        }
    }

    #[test]
    fn a_sandbox_is_refused_to_a_viewer_who_is_not_staff_and_to_the_queue() {
        let env = resolved(sandbox());
        let refused = admit(&env, VIEWER).expect_err("not staff");
        assert_eq!(refused.environment, sandbox());
        assert_eq!(refused.reason, RefusedReason::NotStaff);
        let refused = admit(&env, Entrance::Queued).expect_err("queued");
        assert_eq!(refused.reason, RefusedReason::QueuedOutsideProduction);
        assert!(
            refused.message().contains("dev-luong"),
            "{}",
            refused.message()
        );
        assert_eq!(refused.into_response().status(), StatusCode::FORBIDDEN);
    }
}
