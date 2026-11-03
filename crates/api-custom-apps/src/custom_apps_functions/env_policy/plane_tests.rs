//! The plane half of the registry: which plane each row decides by, and the
//! one rule that holds outside production for every op under every home.

use std::collections::BTreeSet;

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::*;
use crate::custom_apps_planes::{Plane, PlaneSpec, ResolvedPlanes};

/// A row that decides by home names the plane its op really lives on, and
/// only a plane that has a home, or the Airhouse plane whose mode decides
/// `ctx.airhouse.query`, may do so.
#[test]
fn by_home_rows_name_their_own_plane() {
    let with_a_home = [
        Plane::Oltp,
        Plane::Storage,
        Plane::Secrets,
        Plane::Messages,
        Plane::Fetch,
    ];
    for op in HostOp::ALL {
        if let Staged::ByHome(plane) = op.spec().staging {
            assert_eq!(plane, op.plane(), "{op:?}");
            assert!(
                with_a_home.contains(&plane) || plane == Plane::Airhouse,
                "{op:?}: {plane:?} has no home"
            );
        }
    }
}

/// Outside production, an op that is `Allow` under any home is a read (or the
/// rollback of a transaction, which writes nothing) — nothing else may run
/// unchanged in staging or a sandbox.
#[test]
fn non_production_allow_is_reads_only() {
    let environments = [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a dev slot"),
    ];
    let homes = super::golden::home_samples();
    let allowed: BTreeSet<&str> = HostOp::ALL
        .iter()
        .filter(|op| {
            environments.iter().any(|env| {
                homes.iter().any(|(_, home)| {
                    EnvPolicy::for_environment(env.clone())
                        .with_oltp_home(home.clone())
                        .decide(**op)
                        == Decision::Allow
                })
            })
        })
        .map(|op| op.name())
        .collect();
    let expected: BTreeSet<&str> = [
        "query",
        "query_stream",
        "semantic.query",
        "warehouse.query",
        "airhouse.query",
        "org.people",
        "org.places",
        "org.assignments",
        "platform.facts",
        "tx.rollback",
    ]
    .into_iter()
    .collect();
    assert_eq!(allowed, expected);
}

/// `airway.run` sits on the workspace-actions plane, but Airway acts on the
/// project, so no mode may turn its refusal into a hold (design §3 row 12).
#[test]
fn airway_run_is_refused_whatever_the_mode() {
    assert_eq!(
        HostOp::AirwayRun.spec().staging,
        Staged::Decided(Decision::Refuse { fix: AIRWAY_FIX })
    );
    for env in [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a dev slot"),
    ] {
        assert!(
            matches!(
                EnvPolicy::for_environment(env.clone()).decide(HostOp::AirwayRun),
                Decision::Refuse { .. }
            ),
            "{env:?}"
        );
    }
}

#[test]
fn every_plane_key_is_unique() {
    let keys: BTreeSet<&str> = Plane::ALL.iter().map(|p| p.key()).collect();
    assert_eq!(keys.len(), Plane::ALL.len());
    for key in keys {
        assert!(
            !key.is_empty()
                && !key.starts_with('-')
                && !key.ends_with('-')
                && key.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "{key} is not kebab-case"
        );
    }
}

/// `EnvPolicy` does not read the `semantic` or `workspace-actions` modes: no
/// host op's decision depends on them. `semantic.query` is `Allow` and
/// `airway.run` is `Refuse` whatever a spec says, and these are the only ops
/// on those planes. So **every** mode of either plane decides, for **every**
/// host op under every OLTP home, exactly as the defaults do — and
/// `airway.run` stays `Decided(Refuse)` under each of them: a preview run is
/// the automation route's dry run (`automation_run::preview_mode`), never a
/// function calling `airway.run`.
///
/// A future mode-dependent arm (say a `Preview` row that lets `airway.run`
/// through) fails here, because the policies below are built with the mode
/// applied, not with the defaults.
#[test]
fn new_modes_decide_as_defaults() {
    use crate::custom_apps_planes::{PlaneSpec, SemanticMode, WorkspaceActionsMode};

    let on = |plane: Plane| -> Vec<HostOp> {
        HostOp::ALL
            .iter()
            .copied()
            .filter(|op| op.plane() == plane)
            .collect()
    };
    assert_eq!(on(Plane::Semantic), [HostOp::SemanticQuery]);
    assert_eq!(on(Plane::WorkspaceActions), [HostOp::AirwayRun]);

    // Every wire spelling of every mode, with the mode it must resolve to.
    let semantic = [
        ("build-pin", SemanticMode::BuildPin),
        ("branch:feat-x", SemanticMode::Branch),
    ];
    let workspace_actions = [
        ("held", WorkspaceActionsMode::Held),
        ("refused", WorkspaceActionsMode::Refused),
        ("preview", WorkspaceActionsMode::Preview),
    ];
    let homes = super::golden::home_samples();
    let mut built = 0;
    for env in [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a dev slot"),
    ] {
        for (semantic_wire, semantic_mode) in semantic {
            for (actions_wire, actions_mode) in workspace_actions {
                let spec = PlaneSpec::parse(&serde_json::json!({
                    "semantic": semantic_wire,
                    "workspace-actions": actions_wire,
                }))
                .expect("a valid spec");
                let resolved = spec.resolve(&env).expect("planes outside production");
                assert_eq!(resolved.semantic, semantic_mode);
                assert_eq!(resolved.workspace_actions, actions_mode);
                let label = format!("{env:?} semantic={semantic_wire} actions={actions_wire}");

                for (home_name, home) in &homes {
                    let with_mode = EnvPolicy::for_environment(env.clone())
                        .with_planes(resolved.clone())
                        .with_oltp_home(home.clone());
                    let defaults =
                        EnvPolicy::for_environment(env.clone()).with_oltp_home(home.clone());
                    for op in HostOp::ALL {
                        assert_eq!(
                            with_mode.decide(*op),
                            defaults.decide(*op),
                            "{label} home={home_name}: {op:?} decides as the defaults"
                        );
                    }
                    assert_eq!(
                        with_mode.decide(HostOp::SemanticQuery),
                        Decision::Allow,
                        "{label}"
                    );
                    assert!(
                        matches!(with_mode.decide(HostOp::AirwayRun), Decision::Refuse { .. }),
                        "{label}: airway.run is refused under every mode, got {:?}",
                        with_mode.decide(HostOp::AirwayRun)
                    );
                }
                built += 1;
            }
        }
    }
    assert_eq!(
        built, 12,
        "2 environments x 2 semantic x 3 workspace-actions"
    );
}

/// A policy carries the planes of its kind's defaults until admission reads
/// the row; production has none, and cannot be given any.
#[test]
fn a_policy_carries_the_default_planes_of_its_kind() {
    use crate::custom_apps_planes::PlaneSpec;
    assert_eq!(EnvPolicy::production().planes(), None);
    for environment in [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a sandbox"),
    ] {
        let defaults = PlaneSpec::default().resolve(&environment);
        assert!(defaults.is_some());
        let policy = EnvPolicy::for_environment(environment);
        assert_eq!(policy.planes(), defaults.as_ref());
    }
}

#[test]
fn with_planes_replaces_outside_production_and_is_ignored_in_it() {
    use crate::custom_apps_planes::{PlaneSpec, WorkspaceActionsMode};
    let chosen = PlaneSpec::parse(&serde_json::json!({"workspace-actions": "refused"}))
        .expect("a valid spec");

    let staging = EnvPolicy::for_environment(AppEnvironment::Staging);
    let resolved = chosen.resolve(&AppEnvironment::Staging).expect("planes");
    assert_eq!(resolved.workspace_actions, WorkspaceActionsMode::Refused);
    let policy = staging.with_planes(resolved.clone());
    assert_eq!(policy.planes(), Some(&resolved));

    let production = EnvPolicy::production().with_planes(resolved);
    assert_eq!(production.planes(), None);
    assert_eq!(production, EnvPolicy::production());
}

const SQUARE_SANDBOX: &str = "connect.squareupsandbox.com";

/// `fetch: allow` listing [`SQUARE_SANDBOX`], resolved for `environment`.
fn allow_planes(environment: &AppEnvironment) -> ResolvedPlanes {
    PlaneSpec::parse(&serde_json::json!({
        "fetch": {"mode": "allow", "hosts": [SQUARE_SANDBOX]}
    }))
    .expect("a valid spec")
    .resolve(environment)
    .expect("a non-production environment has planes")
}

fn allowing(environment: AppEnvironment) -> EnvPolicy {
    let planes = allow_planes(&environment);
    EnvPolicy::for_environment(environment).with_planes(planes)
}

fn non_production_environments() -> [AppEnvironment; 2] {
    [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a sandbox"),
    ]
}

/// `fetch: allow` moves one row and no other: `ctx.fetch` names the provider
/// sandbox, and every other op decides as it does under the default planes.
#[test]
fn fetch_allow_changes_only_the_fetch_row() {
    for environment in non_production_environments() {
        let default = EnvPolicy::for_environment(environment.clone());
        let allow = allowing(environment.clone());
        assert_eq!(default.decide(HostOp::Fetch), Decision::Hold);
        assert_eq!(
            allow.decide(HostOp::Fetch),
            Decision::Isolate(Target::ProviderSandbox),
            "{environment:?}"
        );
        for op in HostOp::ALL.iter().filter(|op| **op != HostOp::Fetch) {
            assert_eq!(allow.decide(*op), default.decide(*op), "{op:?}");
        }
    }
}

/// Outside production no mode answers `Allow` for `ctx.fetch`.
#[test]
fn fetch_never_decides_allow_outside_production() {
    for environment in non_production_environments() {
        for policy in [
            EnvPolicy::for_environment(environment.clone()),
            allowing(environment),
        ] {
            assert_ne!(policy.decide(HostOp::Fetch), Decision::Allow);
        }
    }
}

/// Production has no planes, so the setting is ignored there, and a production
/// run reading a branch holds fetch whatever planes it was handed.
#[test]
fn fetch_allow_does_not_reach_production_or_a_branch_read() {
    let planes = allow_planes(&AppEnvironment::Staging);
    let production = EnvPolicy::production().with_planes(planes);
    assert_eq!(production.decide(HostOp::Fetch), Decision::Allow);
    let pinned = production.within_scope_pin(Some(uuid::Uuid::from_u128(9)));
    assert_eq!(pinned.decide(HostOp::Fetch), Decision::Hold);
    assert_eq!(pinned.decide_on_branch(HostOp::Fetch), Decision::Hold);
    let unpinned = EnvPolicy::production();
    assert_eq!(unpinned.decide_on_branch(HostOp::Fetch), Decision::Hold);
}

fn post(
    policy: &EnvPolicy,
    url: &str,
    carries_fallback_secret: bool,
) -> (Decision, Option<&'static str>) {
    let url = url::Url::parse(url).expect("a URL");
    let companies = BTreeSet::new();
    let call = FetchCall {
        method: "POST",
        url: &url,
        minted: &[],
        carries_fallback_secret,
        api_key: None,
        overrides_host: false,
        duplicate_authorization: false,
        sandbox_companies: &companies,
    };
    policy.decide_on_fetch(uuid::Uuid::from_u128(7), &call)
}

/// End to end through `decide_on_fetch`: the listed host is sent, a registered
/// host the environment did not list is held with its note, a fallback
/// credential holds, and the default planes hold with no note.
#[test]
fn decide_on_fetch_reads_the_environments_listed_hosts() {
    let square = "https://connect.squareupsandbox.com/v2/payments";
    let stripe = "https://api.stripe.com/v1/charges";
    for environment in non_production_environments() {
        let allow = allowing(environment.clone());
        assert_eq!(
            post(&allow, square, false),
            (Decision::Isolate(Target::ProviderSandbox), None)
        );
        assert_eq!(
            post(&allow, stripe, false),
            (Decision::Hold, Some(NOTE_NOT_LISTED))
        );
        assert_eq!(
            post(&allow, square, true),
            (Decision::Hold, Some(NOTE_FALLBACK_CREDENTIAL))
        );
        let default = EnvPolicy::for_environment(environment);
        assert_eq!(post(&default, square, false), (Decision::Hold, None));
    }
}

/// A stored bare `"allow"` has no hosts (`validate_fetch` refuses to save it,
/// but a row written another way must still fail closed): the op names the
/// provider sandbox, and every call is held as not listed.
#[test]
fn a_stored_bare_allow_with_no_hosts_holds_every_call_as_not_listed() {
    for environment in non_production_environments() {
        let planes = PlaneSpec::parse(&serde_json::json!({ "fetch": "allow" }))
            .expect("a bare mode parses")
            .resolve(&environment)
            .expect("a non-production environment has planes");
        assert!(planes.fetch_hosts.is_empty());
        let policy = EnvPolicy::for_environment(environment).with_planes(planes);
        assert_eq!(
            policy.decide(HostOp::Fetch),
            Decision::Isolate(Target::ProviderSandbox)
        );
        for url in [
            "https://connect.squareupsandbox.com/v2/payments",
            "https://api.stripe.com/v1/charges",
        ] {
            assert_eq!(
                post(&policy, url, false),
                (Decision::Hold, Some(NOTE_NOT_LISTED))
            );
        }
    }
}

/// Production never reads the list: it is `Allow`, with no note.
#[test]
fn decide_on_fetch_in_production_is_allow() {
    let call = post(
        &EnvPolicy::production(),
        "https://connect.squareupsandbox.com/v2/payments",
        false,
    );
    assert_eq!(call, (Decision::Allow, None));
}

/// `ctx.airway.run` is refused outside production whatever the
/// `workspace-actions` mode says: a sandbox that holds workspace actions (its
/// default) still cannot start a pipeline.
#[test]
fn airway_run_is_refused_under_every_workspace_actions_mode() {
    use crate::custom_apps_planes::{PlaneSpec, WorkspaceActionsMode};

    for environment in [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a dev slot"),
    ] {
        for mode in WorkspaceActionsMode::ALL {
            let spec = PlaneSpec::parse(&serde_json::json!({"workspace-actions": mode.as_str()}))
                .expect("a valid spec");
            let planes = spec.resolve(&environment).expect("planes");
            let policy = EnvPolicy::for_environment(environment.clone()).with_planes(planes);
            assert!(
                matches!(
                    policy.decide(HostOp::AirwayRun),
                    Decision::Refuse { fix } if fix == AIRWAY_FIX
                ),
                "{environment} {mode:?}"
            );
        }
    }
}
