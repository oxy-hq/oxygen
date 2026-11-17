//! Who is refused an app's data, and why: every rule of the gate, over facts,
//! and the two credential refusals through the real entry point (they are
//! decided before the database is reached, so no database is needed here).

use axum::http::StatusCode;
use uuid::Uuid;

use super::*;
use crate::custom_apps_agent_fixture as fixture;

fn production() -> AppEnvironment {
    AppEnvironment::Production
}

fn sandbox() -> AppEnvironment {
    AppEnvironment::parse("dev-ana").expect("a sandbox name")
}

/// Everything a staff app admin with non-production reach holds, nothing locked.
fn admin() -> Facts {
    Facts {
        workspace_locked: false,
        org_locked: false,
        app_admin: true,
        non_production_reach: true,
        sandbox_exists: true,
    }
}

fn refusal(result: Result<(), ScopeError>) -> (StatusCode, &'static str) {
    let e = result.expect_err("a refusal");
    (e.status, e.code)
}

#[tokio::test]
async fn a_publish_token_is_refused_before_anything_is_read() {
    let user = fixture::user(None);
    let refused = admit(&user, true, Uuid::new_v4(), Some("staging"))
        .await
        .err()
        .expect("a publish token is refused");
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.code, "publish_token_refused");
}

#[tokio::test]
async fn a_sandbox_agent_token_is_refused_even_for_its_own_sandbox() {
    let (org, app, workspace) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let credential = fixture::credential(Uuid::new_v4(), org, app, workspace);
    let user = fixture::user(Some(credential));
    let refused = admit(&user, false, app, Some("dev-ana"))
        .await
        .err()
        .expect("a sandbox agent token is refused");
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.code, "sandbox_token_refused");
}

#[test]
fn a_publish_token_is_refused_whatever_else_it_carries() {
    assert_eq!(
        refusal(refuse_credential(true, true)),
        (StatusCode::FORBIDDEN, "publish_token_refused")
    );
    assert!(refuse_credential(false, false).is_ok());
}

/// The org locked Oxy staff out of the app's workspace: no environment's data
/// is read, even by an app admin with every other reach.
#[test]
fn a_workspace_lockdown_refuses_an_app_admin_in_every_environment() {
    let facts = Facts {
        workspace_locked: true,
        ..admin()
    };
    for environment in [production(), AppEnvironment::Staging, sandbox()] {
        let (status, code) = refusal(decide(&environment, &facts));
        assert_eq!((status, code), (StatusCode::FORBIDDEN, "oxy_locked_out"));
    }
}

/// A lockdown of another of the org's workspaces still refuses: the OLTP
/// database is the org's, and the app's rows are in it.
#[test]
fn a_lockdown_anywhere_in_the_org_refuses() {
    let facts = Facts {
        org_locked: true,
        ..admin()
    };
    let e = decide(&production(), &facts).expect_err("refused");
    assert_eq!(e.code, "oxy_locked_out");
    assert!(
        e.message.contains("every workspace shares"),
        "{}",
        e.message
    );
}

#[test]
fn the_lockdown_is_named_before_the_missing_role() {
    let facts = Facts {
        workspace_locked: true,
        app_admin: false,
        ..Facts::default()
    };
    assert_eq!(refusal(decide(&production(), &facts)).1, "oxy_locked_out");
}

/// Staff reach without `Ring::AppAdmin` reads no environment, production
/// included: an app's data is its admins'.
#[test]
fn every_environment_needs_the_app_admin_ring() {
    let facts = Facts {
        app_admin: false,
        ..admin()
    };
    for environment in [production(), AppEnvironment::Staging, sandbox()] {
        assert_eq!(
            refusal(decide(&environment, &facts)),
            (StatusCode::FORBIDDEN, "app_admin_required")
        );
    }
}

#[test]
fn staging_and_sandboxes_need_non_production_reach_and_production_does_not() {
    let facts = Facts {
        non_production_reach: false,
        ..admin()
    };
    assert!(decide(&production(), &facts).is_ok());
    for environment in [AppEnvironment::Staging, sandbox()] {
        assert_eq!(
            refusal(decide(&environment, &facts)),
            (StatusCode::FORBIDDEN, "non_production_refused")
        );
    }
}

#[test]
fn a_sandbox_that_does_not_exist_is_not_found() {
    let facts = Facts {
        sandbox_exists: false,
        ..admin()
    };
    assert_eq!(
        refusal(decide(&sandbox(), &facts)),
        (StatusCode::NOT_FOUND, "environment_not_found")
    );
    // Staging is not a sandbox row, so the fact does not apply to it.
    assert!(decide(&AppEnvironment::Staging, &facts).is_ok());
}

/// The whole rule as one truth table: admitted exactly when nothing is
/// locked, the caller is the app's admin, and outside production also has
/// reach and (for a sandbox) a sandbox to read.
#[test]
fn the_rule_admits_exactly_the_unlocked_app_admin_with_reach() {
    for bits in 0u8..32 {
        let facts = Facts {
            workspace_locked: bits & 1 != 0,
            org_locked: bits & 2 != 0,
            app_admin: bits & 4 != 0,
            non_production_reach: bits & 8 != 0,
            sandbox_exists: bits & 16 != 0,
        };
        for environment in [production(), AppEnvironment::Staging, sandbox()] {
            let unlocked = !facts.workspace_locked && !facts.org_locked;
            let reach = match environment {
                AppEnvironment::Production => true,
                AppEnvironment::Staging => facts.non_production_reach,
                AppEnvironment::Dev { .. } => facts.non_production_reach && facts.sandbox_exists,
            };
            let expected = unlocked && facts.app_admin && reach;
            assert_eq!(
                decide(&environment, &facts).is_ok(),
                expected,
                "{environment} with {facts:?}"
            );
        }
    }
}
