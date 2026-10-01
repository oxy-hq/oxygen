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
//! **Phase 3: production, and staging for Oxy staff.** Production is admitted
//! for every caller, with a policy that allows every op — byte-identical to
//! before environments. Staging is admitted only for a **route** call from a
//! viewer who may open non-production (`AppNonProduction`, the rule the staging
//! host uses to serve staging HTML), and its policy holds every write
//! (`env_policy`): nothing a staging function does reaches production, which is
//! invariant 2 held by the host rather than by refusing the run. A staging run
//! reads production data, runs the staging build, and sees
//! `ctx.channel == "staging"`.
//!
//! Still refused: staging from a queued path (schedules, webhooks and Run now
//! stay production-only — their identities carry no environment yet, design
//! §4.2), and every dev slot (Phase 4 of the environments design).
//!
//! Phases 4 and 5 of the previews plan change what staging's policy *does*
//! (`Hold` → `Isolate`), not who is admitted, so they land in `env_policy`,
//! not here. The one fact they need from the database is decided here, once
//! per admitted run: whether the org has an OLTP staging branch
//! ([`with_oltp_home`], P4b).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::env_policy::{EnvPolicy, OltpHome};
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
    /// Staging is Oxy staff's; this viewer is not.
    NotStaff,
    /// Staging runs only on a staff route call.
    QueuedOutsideProduction,
    /// Dev slots do not run functions yet.
    DevSlot,
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
            RefusedReason::DevSlot => format!(
                "functions do not run in the {env} environment yet: its writes are not \
                 isolated from production. Call the function in staging or production"
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
    match (&resolved.environment, entrance) {
        (AppEnvironment::Production, _)
        | (
            AppEnvironment::Staging,
            Entrance::Route {
                non_production_reach: true,
            },
        ) => Ok(Admission {
            environment: resolved.clone(),
            policy: EnvPolicy::for_environment(resolved.environment.clone()),
        }),
        (AppEnvironment::Staging, Entrance::Route { .. }) => Err(refused(RefusedReason::NotStaff)),
        (AppEnvironment::Staging, Entrance::Queued) => {
            Err(refused(RefusedReason::QueuedOutsideProduction))
        }
        (AppEnvironment::Dev { .. }, _) => Err(refused(RefusedReason::DevSlot)),
    }
}

/// `admission`, with where its `ctx.oltp` lands decided — once, here, for the
/// whole invocation (env design §4.2; `env_policy::OltpHome`). Production
/// always writes its own database and pays no lookup. Outside production, one
/// lookup of the org's staging branch ([`oltp_home_for`]).
pub(crate) async fn with_oltp_home(
    db: &sea_orm::DatabaseConnection,
    mut admission: Admission,
    org_id: Uuid,
) -> Result<Admission, sea_orm::DbErr> {
    if admission.policy.is_production() {
        return Ok(admission);
    }
    let (_, branch) = oxy_oltp::branches::find(db, org_id, oxy_oltp::OltpBranch::Staging).await?;
    admission.policy = admission
        .policy
        .with_oltp_home(oltp_home_for(branch.as_ref()));
    Ok(admission)
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

    /// A dev slot's writes have nowhere isolated to go and it has no hold
    /// policy of its own, so it does not run at all — even for staff.
    #[test]
    fn a_dev_slot_is_refused_even_to_staff() {
        let dev = AppEnvironment::Dev {
            handle: "luong".into(),
        };
        for entrance in [STAFF, VIEWER, Entrance::Queued] {
            let refused = admit(&resolved(dev.clone()), entrance).expect_err("dev slot");
            assert_eq!(refused.environment, dev);
            assert_eq!(refused.reason, RefusedReason::DevSlot);
        }
    }
}
