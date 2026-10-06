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

const OWN: Entrance = Entrance::SandboxAgent { own_sandbox: true };
const NOT_OWN: Entrance = Entrance::SandboxAgent { own_sandbox: false };

/// A sandbox agent token runs a function in a sandbox it created, under that
/// sandbox's policy — the one staff get there.
#[test]
fn a_sandbox_agent_token_runs_in_its_own_sandbox() {
    let admission = admit(&resolved(sandbox()), OWN).expect("its own sandbox");
    assert_eq!(admission.policy.environment(), &sandbox());
    assert!(!admission.policy.is_production());
    let staff = admit(&resolved(sandbox()), STAFF).expect("staff");
    assert_eq!(admission, staff, "the same admission staff get");
}

/// The second refusal of a production call (decision 6): production is
/// admitted to every other entrance, and to this one never — whatever was
/// decided about ownership before the gate. Staging, and a sandbox that is
/// not the token's, are refused the same way: a bare `404`.
#[test]
fn a_sandbox_agent_token_is_refused_everywhere_else_production_included() {
    for environment in [AppEnvironment::Production, AppEnvironment::Staging] {
        for entrance in [OWN, NOT_OWN] {
            let refused =
                admit(&resolved(environment.clone()), entrance).expect_err("never the token's");
            assert_eq!(
                refused.reason,
                RefusedReason::NotOwnSandbox,
                "{environment}"
            );
            assert_eq!(refused.into_response().status(), StatusCode::NOT_FOUND);
        }
    }
    let refused = admit(&resolved(sandbox()), NOT_OWN).expect_err("another creator's");
    assert_eq!(refused.reason, RefusedReason::NotOwnSandbox);
    assert_eq!(refused.into_response().status(), StatusCode::NOT_FOUND);
}
