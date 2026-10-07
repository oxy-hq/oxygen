//! The function gate for a sandbox agent token that names **staging**
//! (`Entrance::StagingAgent`), beside `environment_gate_tests`, whose cases —
//! a token minted without staging among them — stand unchanged.

use super::*;

fn resolved(environment: AppEnvironment) -> ResolvedEnvironment {
    ResolvedEnvironment {
        environment,
        build_id: Some(Uuid::from_u128(1)),
    }
}

fn sandbox() -> AppEnvironment {
    AppEnvironment::parse("dev-a").expect("a sandbox")
}

const GRANTED: Entrance = Entrance::StagingAgent { granted: true };
const NOT_GRANTED: Entrance = Entrance::StagingAgent { granted: false };

/// Granted staging, the token runs there under staging's own policy: the
/// admission a staff route call gets, hold for hold.
#[test]
fn a_token_granted_staging_is_admitted_as_a_staff_call_there_is() {
    let staging = resolved(AppEnvironment::Staging);
    let admitted = admit(&staging, GRANTED).expect("admitted to staging");
    assert_eq!(admitted.environment, staging);
    assert!(!admitted.policy.is_production());
    let staff = Entrance::Route {
        non_production_reach: true,
    };
    assert_eq!(admitted, admit(&staging, staff).expect("staff on staging"));
}

/// The staging entrance opens staging and nothing else: production — which
/// every other caller is admitted to — and a sandbox are refused it, granted
/// or not, and staging without the grant is refused.
#[test]
fn the_staging_entrance_opens_nothing_but_granted_staging() {
    for environment in [AppEnvironment::Production, sandbox()] {
        for entrance in [GRANTED, NOT_GRANTED] {
            let refused = admit(&resolved(environment.clone()), entrance).expect_err("refused");
            assert_eq!(
                refused.reason,
                RefusedReason::NotOwnSandbox,
                "{environment}"
            );
        }
    }
    let refused = admit(&resolved(AppEnvironment::Staging), NOT_GRANTED).expect_err("refused");
    assert_eq!(refused.reason, RefusedReason::NotOwnSandbox);
    // Told as an unknown app is: the token's `404`, not the staff `403`.
    let response = refused.into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A token minted without staging takes the entrance it always took, and is
/// refused staging by it whatever it owns elsewhere.
#[test]
fn the_sandbox_entrance_still_opens_no_staging() {
    let staging = resolved(AppEnvironment::Staging);
    for own_sandbox in [true, false] {
        let refused = admit(&staging, Entrance::SandboxAgent { own_sandbox }).expect_err("refused");
        assert_eq!(refused.reason, RefusedReason::NotOwnSandbox);
    }
}
