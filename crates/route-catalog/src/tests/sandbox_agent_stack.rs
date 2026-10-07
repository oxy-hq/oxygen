//! Stack coverage for the sandbox agent token (sandbox agent credential
//! design §3.2 and §7.2).
//!
//! A row of the route allow-list is honoured only if **every** decision its
//! stack makes — the mount's layers, then the handler — is one the token's
//! reach covers. The model covers four actions and refuses the rest, so a
//! layer added to the console that decides a fifth would 403 the whole loop
//! and say nothing about why. This restates §3.2's table, row by row, and
//! holds it to three things: the allow-list, the model, and the layers the
//! console actually mounts.

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

/// What a decision is asked of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum On {
    /// The platform singleton: the console's two doors.
    Platform,
    /// "A sandbox that does not exist yet": listing and creating.
    NewSandbox,
    /// A sandbox the token created, of an app it is granted.
    OwnSandbox,
}

type Decision = (Action, On);

const CONSOLE: [Decision; 2] = [
    (Action::PlatformOps, On::Platform),
    (Action::PlatformApps, On::Platform),
];

/// §3.2's table: what the whole stack of each row decides. F1 and L1 are the
/// two rows the allow-list table does not carry (the serve tree and the
/// public router); they have a stack like any other.
fn stack_of(row: &str) -> Vec<Decision> {
    let own = (Action::AppNonProduction, On::OwnSandbox);
    let console = |handler: Decision| CONSOLE.into_iter().chain([handler]).collect();
    match row {
        "A1" | "A2" => Vec::new(),
        "E1" | "E2" => console((Action::AppNonProduction, On::NewSandbox)),
        "E3" | "C1" | "C2" | "C3" | "R1" | "R2" | "R3" | "S1" | "S2" | "S3" => console(own),
        "P1" => vec![own],
        "F1" | "L1" => vec![own, (Action::AppAdmin, On::OwnSandbox)],
        other => panic!(
            "{other} is a row of the sandbox agent token's allow-list with no stated stack: \
             say here what its layers and its handler decide, and check the token covers it"
        ),
    }
}

fn every_row() -> Vec<&'static str> {
    let mut rows: Vec<&str> = SANDBOX_ALLOWED.iter().map(|allowed| allowed.row).collect();
    rows.extend(["F1", "L1"]);
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// A Global Admin's facts as a request on the token carries them: the widest
/// minter, so what is refused below is refused by the token and nothing else.
fn token_facts() -> PrincipalFacts {
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
                staging: false,
            }],
        }),
    })
}

fn resource(on: On) -> Resource {
    let own = EnvFacet::Sandbox {
        created_by_token: Some(TOKEN),
    };
    match on {
        On::Platform => Resource::platform(),
        On::NewSandbox => Resource::app_environment(APP, ORG, EnvFacet::NewSandbox),
        On::OwnSandbox => Resource::app_environment(APP, ORG, own).published_from(WORKSPACE),
    }
}

/// Every resource a decision of the loop could be asked of, and the ones it
/// must not be: production, staging, a colleague's sandbox, no environment.
fn any_resource() -> Vec<Resource> {
    let colleague = EnvFacet::Sandbox {
        created_by_token: None,
    };
    vec![
        resource(On::Platform),
        resource(On::NewSandbox),
        resource(On::OwnSandbox),
        Resource::app(APP, ORG),
        Resource::app_environment(APP, ORG, EnvFacet::Production),
        Resource::app_environment(APP, ORG, EnvFacet::Staging),
        Resource::app_environment(APP, ORG, colleague),
        Resource::org(ORG),
        Resource::workspace(WORKSPACE, ORG),
    ]
}

/// The token is admitted by every decision its own loop makes.
#[test]
fn the_token_covers_every_decision_each_rows_stack_makes() {
    let token = token_facts();
    for row in every_row() {
        for (action, on) in stack_of(row) {
            assert!(
                allows(&token, action, &resource(on)),
                "{row}: its stack decides {action:?} on {on:?}, which the token does not cover — \
                 the loop is refused there"
            );
        }
    }
}

/// The loop needs exactly what the model covers: the four actions, and no
/// fifth the token holds for nothing.
#[test]
fn the_loop_decides_the_four_covered_actions_and_no_other() {
    let decided: BTreeSet<String> = every_row()
        .into_iter()
        .flat_map(stack_of)
        .map(|(action, _)| format!("{action:?}"))
        .collect();
    let token = token_facts();
    let covered: BTreeSet<String> = Action::ALL
        .into_iter()
        .filter(|action| any_resource().iter().any(|r| allows(&token, *action, r)))
        .map(|action| format!("{action:?}"))
        .collect();
    assert_eq!(decided, covered);
    assert_eq!(decided.len(), 4, "{decided:?}");
}

/// A decision of the loop holds only on the resource its row asks about: the
/// tenant ones never on production, staging, a colleague's sandbox, or an app
/// with no environment named.
#[test]
fn a_rows_decision_does_not_hold_off_its_own_resource() {
    let token = token_facts();
    let colleague = EnvFacet::Sandbox {
        created_by_token: None,
    };
    let elsewhere = [
        Resource::app(APP, ORG),
        Resource::app_environment(APP, ORG, EnvFacet::Production),
        Resource::app_environment(APP, ORG, EnvFacet::Staging),
        Resource::app_environment(APP, ORG, colleague),
    ];
    for action in [Action::AppNonProduction, Action::AppAdmin] {
        for resource in &elsewhere {
            assert!(
                !allows(&token, action, resource),
                "{action:?} on {resource:?}"
            );
        }
    }
    assert!(!allows(&token, Action::AppAdmin, &resource(On::NewSandbox)));
    for action in [Action::PlatformOps, Action::PlatformApps] {
        assert!(!allows(&token, action, &Resource::org(ORG)), "{action:?}");
    }
}

fn source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `/customer-apps` nest of `router/global.rs`: from its mount to the
/// label that closes it.
fn console_nest(router: &str) -> &str {
    let start = router
        .find("\"/customer-apps\",\n            RouteRole::FleetOk,")
        .expect("the /customer-apps nest is mounted in router/global.rs");
    let end = router[start..]
        .find("\"app registration rows plus S3 bundles\"")
        .expect("the nest's closing label");
    &router[start..start + end]
}

/// The console's layers are the four the table's `CONSOLE` rows were written
/// from: two decide a platform action each, two decide none. A fifth layer
/// fails here until someone says what it decides — and if that is an action
/// the token does not cover, the loop is refused on every console row.
#[test]
fn the_console_mounts_the_layers_the_table_was_written_from() {
    let router = source("app/src/server/router/global.rs");
    let nest = console_nest(&router);
    let layers = nest.matches(".layer(middleware::from_fn(").count();
    assert_eq!(
        layers, 4,
        "the /customer-apps nest has {layers} layers: state in `stack_of` what the new one \
         decides, for every console row"
    );
    for layer in [
        "admin::assume::block_admin_while_acting",
        "app_scope_guard::enforce_app_scope",
        "platform_cap_guard::require(\n                    crate::server::authz::Action::PlatformApps,",
        "oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware",
    ] {
        assert!(nest.contains(layer), "the nest no longer mounts {layer}");
    }
    assert_eq!(nest.matches("platform_cap_guard::require(").count(), 1);
    let door = source("app/src/server/api/middlewares/oxy_owner_or_app_admin_guard.rs");
    assert!(
        door.contains("crate::server::authz::Action::PlatformOps"),
        "the outer door no longer decides PlatformOps"
    );
}

/// P1 is mounted after the console's layers, so they do not cover it: its
/// stack is the handler's decision alone, as the table says.
#[test]
fn the_publish_route_is_mounted_outside_the_console_layers() {
    let router = source("app/src/server/router/global.rs");
    let nest = console_nest(&router);
    let last_layer = nest
        .rfind(".layer(middleware::from_fn(")
        .expect("the console layers");
    let publish = nest
        .find("post(crate::server::api::custom_apps_publish::publish_handler)")
        .expect("the publish route is in the nest");
    assert!(publish > last_layer, "P1 moved under the console layers");
    assert!(
        stack_of("P1")
            .iter()
            .all(|(action, _)| *action == Action::AppNonProduction)
    );
    // Every other console row is registered before the layers, so they apply.
    let environments = nest
        .find("\"/{id}/environments\"")
        .expect("the sandbox routes");
    assert!(environments < last_layer);
}
