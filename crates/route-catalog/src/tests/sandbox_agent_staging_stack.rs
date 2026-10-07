//! Stack coverage for the **staging option** of a sandbox agent token
//! (sandbox agent credential design, "Staging option"), beside
//! `sandbox_agent_stack`, which states the stack of each row for a token
//! minted without it.
//!
//! Staging adds no route and no action: a token granted an app's staging uses
//! rows of the same allow-list with `staging` where it named a sandbox. So
//! every row is said here to be one of three things — it runs on staging for
//! such a token, it stays a sandbox's alone, or it names no one environment —
//! and a row added to the allow-list fails until it is classified.

use std::collections::BTreeSet;
use std::path::PathBuf;

use oxy_app::server::api::middlewares::app_grant_scope::SANDBOX_ALLOWED;
use oxy_app::server::authz::{
    Action, EnvFacet, PlatformRole, PlatformStanding, PrincipalFacts, Resource, RoleCeiling,
    SandboxAgentReach, SandboxApp, Scope, TokenGrant, TokenReach, allows,
};
use uuid::Uuid;

const ORG: Uuid = Uuid::from_u128(0x0A);
const WORKSPACE: Uuid = Uuid::from_u128(0x0B);
const APP: Uuid = Uuid::from_u128(0x0C);
const TOKEN: Uuid = Uuid::from_u128(0x70);

/// Rows a token granted staging also uses on staging: show the environment,
/// publish a draft, list and run checks, read runs, invocations, held writes
/// and logs, and call a function. F1 and L1 are the serve tree's and the
/// public router's rows, which the allow-list table does not carry.
const ON_STAGING: [&str; 10] = ["C1", "C2", "C3", "E3", "F1", "L1", "P1", "R1", "R2", "R3"];
/// Rows that stay a sandbox's alone for every token, by their handler and not
/// by the model: an environment's secrets.
const SANDBOX_ONLY: [&str; 3] = ["S1", "S2", "S3"];
/// Rows that name no one environment: the token about itself, and listing or
/// creating a sandbox.
const NO_ENVIRONMENT: [&str; 4] = ["A1", "A2", "E1", "E2"];

/// The tenant decisions a row's stack makes, asked of the environment the
/// request names (`sandbox_agent_stack::stack_of`).
fn decisions_of(row: &str) -> Vec<Action> {
    match row {
        "F1" | "L1" => vec![Action::AppNonProduction, Action::AppAdmin],
        _ => vec![Action::AppNonProduction],
    }
}

/// A Global Admin's facts as a request on the token carries them, granted
/// `APP` with or without staging.
fn token_facts(staging: bool) -> PrincipalFacts {
    let minter = PrincipalFacts {
        user_id: Uuid::from_u128(1),
        platform: Some(PlatformStanding {
            role: PlatformRole::GlobalAdmin,
            caps: PlatformRole::GlobalAdmin.caps(),
            scope: Scope::All,
        }),
        ..Default::default()
    };
    minter.narrowed_by(&TokenReach {
        all_access: false,
        platform: true,
        partner: false,
        grants: vec![TokenGrant {
            org_id: ORG,
            workspace_id: Some(WORKSPACE),
            ceiling: RoleCeiling::Admin,
        }],
        blocked_orgs: Vec::new(),
        sandbox_agent: Some(SandboxAgentReach {
            token_id: TOKEN,
            apps: vec![SandboxApp {
                app_id: APP,
                org_id: ORG,
                staging,
            }],
        }),
    })
}

fn app_in(facet: EnvFacet) -> Resource {
    Resource::app_environment(APP, ORG, facet).published_from(WORKSPACE)
}

/// A source file, by its path under `crates/`.
fn source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn every_row_of_the_loop_says_whether_it_runs_on_staging() {
    let mut rows: BTreeSet<&str> = SANDBOX_ALLOWED.iter().map(|allowed| allowed.row).collect();
    rows.extend(["F1", "L1"]);
    let classified: Vec<&str> = ON_STAGING
        .into_iter()
        .chain(SANDBOX_ONLY)
        .chain(NO_ENVIRONMENT)
        .collect();
    let distinct: BTreeSet<&str> = classified.iter().copied().collect();
    assert_eq!(distinct.len(), classified.len(), "a row is in two lists");
    assert_eq!(
        rows, distinct,
        "a row of the sandbox agent token's allow-list is not classified for staging: say in \
         this file whether a token granted staging uses it there, and what refuses it if not"
    );
}

/// Granted staging, the token is admitted on staging by every decision of
/// every row that runs there — and by none of them without the grant.
#[test]
fn the_grant_covers_staging_for_the_rows_that_run_there_and_only_with_it() {
    let (with, without) = (token_facts(true), token_facts(false));
    let staging = app_in(EnvFacet::Staging);
    for row in ON_STAGING {
        for action in decisions_of(row) {
            assert!(
                allows(&with, action, &staging),
                "{row}: its stack decides {action:?} on staging, which the grant does not cover"
            );
            assert!(
                !allows(&without, action, &staging),
                "{row}: {action:?} opened staging for a token minted without it"
            );
        }
    }
}

/// The grant covers production for no row, names no fifth action, and is the
/// app's own: the same id under another org is not staged.
#[test]
fn the_grant_covers_no_production_no_other_action_and_no_other_org() {
    let with = token_facts(true);
    let production = app_in(EnvFacet::Production);
    let unnamed = Resource::app(APP, ORG).published_from(WORKSPACE);
    let elsewhere = Resource::app_environment(APP, Uuid::from_u128(0x0D), EnvFacet::Staging);
    for action in Action::ALL {
        assert!(!allows(&with, action, &production), "{action:?}");
        assert!(!allows(&with, action, &elsewhere), "{action:?}");
        let tenant = matches!(action, Action::AppNonProduction | Action::AppAdmin);
        assert_eq!(
            allows(&with, action, &app_in(EnvFacet::Staging)),
            tenant,
            "{action:?} on staging"
        );
        assert!(!allows(&with, action, &unnamed), "{action:?} with no facet");
    }
}

/// The rows that stay a sandbox's alone are refused staging by their handler.
/// The model would open staging to them — which is why each must say so
/// itself, and why its handler is pinned here by name.
#[test]
fn the_sandbox_only_rows_are_held_by_their_handler() {
    let with = token_facts(true);
    assert!(allows(
        &with,
        Action::AppNonProduction,
        &app_in(EnvFacet::Staging)
    ));
    let secrets = source("app/src/server/api/custom_apps_secrets/agent.rs");
    assert!(
        secrets.contains("if is_sandbox(environment) && may_open_environment("),
        "S1–S3: `secrets::agent::authorize` no longer asks for a sandbox before it asks the \
         model, so a token granted staging would read and write staging's secrets"
    );
    let handlers = source("app/src/server/api/custom_apps_sandboxes/handlers.rs");
    assert!(
        handlers.contains("if deletes_a_fixed_environment(caller, door, &environment) => false"),
        "E3: the delete door no longer refuses a sandbox agent token production and staging"
    );
}
