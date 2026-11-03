//! `PlaneHomes`: the default homes decide as today's table does, and no mode
//! of any plane opens a write to production
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.2).

use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use uuid::Uuid;

use super::tests::is_write;
use super::*;
use crate::custom_apps_planes::{Plane, PlaneSpec, ResolvedPlanes};
use crate::custom_apps_secrets::scope::EnvLookup;

const APP: Uuid = Uuid::from_u128(7);

fn non_production() -> [AppEnvironment; 2] {
    [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a dev slot"),
    ]
}

fn defaults(environment: &AppEnvironment) -> ResolvedPlanes {
    PlaneSpec::default()
        .resolve(environment)
        .expect("a non-production environment has planes")
}

/// `plane` set to `mode` over the defaults of `environment`.
fn with_mode(environment: &AppEnvironment, plane: Plane, mode: &str) -> ResolvedPlanes {
    PlaneSpec::parse(&serde_json::json!({ plane.key(): mode }))
        .unwrap_or_else(|e| panic!("{plane:?} {mode}: {e}"))
        .resolve(environment)
        .expect("a non-production environment has planes")
}

/// The golden table, rendered by policies that carry the resolved default
/// homes, is `golden_decisions.txt` byte for byte: resolving homes decides
/// nothing differently from today.
#[test]
fn the_default_homes_decide_as_the_golden_table_does() {
    let table = super::golden::render_table_of(|environment| {
        EnvPolicy::for_environment(environment.clone()).with_planes(defaults(environment))
    });
    assert!(
        table == include_str!("golden_decisions.txt"),
        "decisions under PlaneHomes::default_for differ from golden_decisions.txt"
    );
}

#[test]
fn a_policys_homes_are_its_kinds_defaults_until_the_row_says_otherwise() {
    for environment in non_production() {
        let policy = EnvPolicy::for_environment(environment.clone());
        assert_eq!(
            policy.homes(),
            PlaneHomes::default_for(environment.kind()).as_ref(),
            "{environment}"
        );
        assert_eq!(
            PlaneHomes::resolve(&defaults(&environment), &environment),
            PlaneHomes::default_for(environment.kind()),
            "{environment}"
        );
    }
    assert_eq!(EnvPolicy::production().homes(), None);
    assert_eq!(
        PlaneHomes::default_for(AppEnvironmentKind::Production),
        None
    );
}

#[test]
fn production_has_no_homes_whatever_it_is_given() {
    let planes = defaults(&AppEnvironment::Staging);
    assert_eq!(
        PlaneHomes::resolve(&planes, &AppEnvironment::Production),
        None
    );
    assert_eq!(EnvPolicy::production().with_planes(planes).homes(), None);
}

#[test]
fn today_s_modes_resolve_to_today_s_homes() {
    for kind in [AppEnvironmentKind::Staging, AppEnvironmentKind::Dev] {
        let homes = PlaneHomes::default_for(kind).expect("homes outside production");
        assert_eq!(homes.storage, StorageHome::SiloWithFallback);
        assert_eq!(homes.secrets, SecretsHome::Chain);
        assert_eq!(homes.messages, MessagesHome::Invoker);
    }
}

#[test]
fn the_workspace_actions_mode_is_the_home() {
    let chosen = |mode: &str| {
        let environment = AppEnvironment::Staging;
        PlaneHomes::resolve(
            &with_mode(&environment, Plane::WorkspaceActions, mode),
            &environment,
        )
        .expect("staging homes")
        .workspace_actions
    };
    assert_eq!(chosen("held"), WorkspaceActionsHome::Held);
    assert_eq!(chosen("refused"), WorkspaceActionsHome::Refused);
}

/// Whether `target` is a home of production's under `policy`: what an
/// `Isolate` must never be. The match is exhaustive, so a target added later
/// must say how it is known to be outside production.
fn lands_in_production(policy: &EnvPolicy, op: HostOp, target: Target) -> bool {
    match target {
        Target::StorageSilo => policy
            .silo_for(op, APP)
            .is_none_or(|silo| silo.is_production()),
        Target::EnvSecrets => policy.secret_segment().is_none(),
        Target::OltpBranch => matches!(policy.oltp_home(), OltpHome::Production),
        Target::Outbox => policy
            .outbox_silo(APP)
            .is_none_or(|silo| silo.is_production()),
        // These name the environment's own database, schema or recipient by
        // construction (`homes`), and are pinned by `differential`.
        Target::MappedDestination
        | Target::SiblingSchema
        | Target::InvokerEmail
        | Target::InvokerNotify => policy.is_production(),
        // A read through the environment's overlay: its copies land in the
        // sibling (`SiblingSchema`), never production's schema.
        Target::AirhouseOverlay | Target::AirhouseSnapshot => policy.is_production(),
        // Only a call to a registered provider sandbox the environment lists
        // is sent (`EnvPolicy::decide_on_fetch`, pinned in `upload_tests`).
        Target::ProviderSandbox => policy.is_production(),
    }
}

/// For every op, under every mode of its plane, in staging and a sandbox and
/// with every `OltpHome`: a write is never `Allow`, and an `Isolate` is never
/// production's home.
#[test]
fn no_mode_of_any_plane_lets_a_write_reach_production() {
    let homes = super::golden::home_samples();
    let mut checked = 0;
    for environment in non_production() {
        for plane in Plane::ALL {
            for mode in plane.modes() {
                // `branch` is the one mode that takes an argument.
                let mode = match (*plane, mode) {
                    (Plane::Semantic, "branch") => "branch:main",
                    _ => mode,
                };
                let planes = with_mode(&environment, *plane, mode);
                for (label, home) in &homes {
                    let policy = EnvPolicy::for_environment(environment.clone())
                        .with_planes(planes.clone())
                        .with_oltp_home(home.clone());
                    for op in HostOp::ALL.iter().filter(|op| op.plane() == *plane) {
                        let decision = policy.decide(*op);
                        let at = format!("{op:?} {plane:?}={mode} {environment} {label}");
                        if is_write(*op) {
                            assert_ne!(decision, Decision::Allow, "{at}");
                        }
                        if let Decision::Isolate(target) = decision {
                            assert!(!lands_in_production(&policy, *op, target), "{at}");
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 0);
}

/// `capture` keeps both message ops in the environment's outbox: an
/// `Isolate`, never a send, and not the invoker's.
#[test]
fn a_capture_home_isolates_both_message_ops_to_the_outbox() {
    for environment in non_production() {
        let policy = EnvPolicy::for_environment(environment.clone()).with_planes(with_mode(
            &environment,
            Plane::Messages,
            "capture",
        ));
        assert_eq!(
            policy.homes().map(|homes| homes.messages),
            Some(MessagesHome::Capture)
        );
        for op in [HostOp::EmailSend, HostOp::Notify] {
            assert_eq!(
                policy.decide(op),
                Decision::Isolate(Target::Outbox),
                "{environment} {op:?}"
            );
        }
    }
}

/// An outbox is the environment's own silo and reads nothing through to
/// production; production has none.
#[test]
fn the_outbox_silo_is_the_environments_own_and_reads_no_fallback() {
    assert_eq!(EnvPolicy::production().outbox_silo(APP), None);
    for environment in non_production() {
        let policy = EnvPolicy::for_environment(environment.clone());
        let silo = policy
            .outbox_silo(APP)
            .expect("an outbox outside production");
        assert_eq!(silo.fallback(), None, "{environment}");
        assert_eq!(
            silo.prefix(),
            policy.storage_silo(APP).prefix(),
            "{environment}"
        );
        assert!(!silo.is_production(), "{environment}");
    }
}

/// The storage mode picks the silo's fallback: `branch` reads through to
/// production's key, `empty` reads the silo alone. Writes land in the same
/// silo either way.
#[test]
fn the_storage_mode_decides_whether_the_silo_reads_through() {
    for environment in non_production() {
        let silo_under = |mode: &str, op: HostOp| {
            EnvPolicy::for_environment(environment.clone())
                .with_planes(with_mode(&environment, Plane::Storage, mode))
                .silo_for(op, APP)
                .expect("an isolated storage op has a silo")
        };
        for op in [HostOp::StorageGet, HostOp::StorageHead, HostOp::StorageCopy] {
            let branch = silo_under("branch", op);
            let empty = silo_under("empty", op);
            assert!(branch.fallback().is_some(), "{environment} {op:?}");
            assert_eq!(empty.fallback(), None, "{environment} {op:?}");
            assert_eq!(empty.prefix(), branch.prefix(), "{environment} {op:?}");
        }
    }
}

/// Under `empty`, the secrets home is the environment's own path: the shared
/// keys of the manifest are not carried, so nothing falls back to production.
#[test]
fn an_empty_secrets_home_carries_no_shared_keys() {
    for environment in non_production() {
        let shared = || ["API_KEY".to_string()];
        let inherit = EnvPolicy::for_environment(environment.clone())
            .with_planes(with_mode(&environment, Plane::Secrets, "inherit"))
            .with_shared_env(shared());
        assert!(inherit.shared_env().contains("API_KEY"), "{environment}");
        let empty = EnvPolicy::for_environment(environment.clone())
            .with_planes(with_mode(&environment, Plane::Secrets, "empty"))
            .with_shared_env(shared());
        assert_eq!(empty.homes().map(|h| h.secrets), Some(SecretsHome::OwnOnly));
        assert!(empty.shared_env().is_empty(), "{environment}");
        assert_eq!(
            empty.homes().expect("homes").secrets.lookup(),
            EnvLookup::OwnOnly
        );
    }
}
