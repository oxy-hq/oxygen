use super::{
    FACTORY_ONLY_KINDS, IDE_DEFER_AIRWAY_ENV, IDE_DEFER_QUEUE_WORK_ENV, IdeDeferral,
    drive_policy_for,
};
use crate::server::role_manifest::Role;
use agentic_pipeline::recovery::{DrivePolicy, STRANDED_GRACE_SECS, may_drive};
use agentic_runtime::coordinator::{AIRWAY_SOURCE_TYPE, COMPILE_SOURCE_TYPE};

const NEITHER: IdeDeferral = IdeDeferral {
    airway: false,
    queue_work: false,
};
const AIRWAY: IdeDeferral = IdeDeferral {
    airway: true,
    queue_work: false,
};
const QUEUE_WORK: IdeDeferral = IdeDeferral {
    airway: false,
    queue_work: true,
};
const BOTH: IdeDeferral = IdeDeferral {
    airway: true,
    queue_work: true,
};
const EVERY_GATE: [IdeDeferral; 4] = [NEITHER, AIRWAY, QUEUE_WORK, BOTH];
const EVERY_ROLE: [Role; 4] = [Role::Ide, Role::Serve, Role::Worker, Role::All];

/// A spread of what `source_type` can hold: the two named kinds, the queued
/// domains, the system kind, host-registered Custom kinds, a kind nobody has
/// written yet, and a row with none.
const KINDS: [Option<&str>; 9] = [
    Some(COMPILE_SOURCE_TYPE),
    Some(AIRWAY_SOURCE_TYPE),
    Some("workflow"),
    Some("analytics"),
    Some("preagg_cycle"),
    Some("app_function"),
    Some("health_eval_workspace"),
    Some("a_kind_added_next_year"),
    None,
];

const FRESH: u64 = 0;

/// Every role × every gate combination, written out. A table rather than a
/// loop over a formula, so the expectation cannot inherit a mistake from the
/// function it is checking.
#[test]
fn every_role_and_gate_combination_resolves_to_its_policy() {
    let all = DrivePolicy::ALL;
    let no_compile = DrivePolicy::Except(&[COMPILE_SOURCE_TYPE]);
    let no_airway = DrivePolicy::Defer(&[AIRWAY_SOURCE_TYPE]);
    let only_compile = DrivePolicy::Only(&[COMPILE_SOURCE_TYPE]);

    let expected = [
        (Role::Ide, NEITHER, all),
        (Role::Ide, AIRWAY, no_airway),
        (Role::Ide, QUEUE_WORK, only_compile),
        (Role::Ide, BOTH, only_compile),
        (Role::All, NEITHER, all),
        (Role::All, AIRWAY, all),
        (Role::All, QUEUE_WORK, all),
        (Role::All, BOTH, all),
        (Role::Worker, NEITHER, no_compile),
        (Role::Worker, AIRWAY, no_compile),
        (Role::Worker, QUEUE_WORK, no_compile),
        (Role::Worker, BOTH, no_compile),
        (Role::Serve, NEITHER, no_compile),
        (Role::Serve, AIRWAY, no_compile),
        (Role::Serve, QUEUE_WORK, no_compile),
        (Role::Serve, BOTH, no_compile),
    ];
    assert_eq!(expected.len(), EVERY_ROLE.len() * EVERY_GATE.len());
    for (role, defer, want) in expected {
        assert_eq!(
            drive_policy_for(role, defer),
            want,
            "{role:?} with {defer:?}"
        );
    }
}

/// The rollout property: a deployment that sets nothing behaves exactly as it
/// did before either gate existed. Stated as behaviour, not as an enum value —
/// with nothing set an `ide` drives every kind the moment it is queued.
#[test]
fn an_ide_that_sets_nothing_drives_every_kind_at_once() {
    let policy = drive_policy_for(Role::Ide, NEITHER);
    for kind in KINDS {
        assert!(
            may_drive(kind, FRESH, policy),
            "{kind:?} must be driven by an ide with no gate set"
        );
    }
}

/// The new gate: only compile is driven at selection, every other kind —
/// named or not — is left for the fleet.
#[test]
fn an_ide_deferring_queue_work_drives_only_compile_at_selection() {
    let policy = drive_policy_for(Role::Ide, QUEUE_WORK);
    for kind in KINDS {
        let is_compile = kind == Some(COMPILE_SOURCE_TYPE);
        assert_eq!(
            may_drive(kind, FRESH, policy),
            is_compile,
            "{kind:?}: an ide deferring queue work takes compile and nothing else"
        );
    }
}

/// The safety net: whatever the deferring ide left, it takes once nobody else
/// has for the grace — so a missing fleet is slow, never stuck.
#[test]
fn an_ide_deferring_queue_work_takes_every_kind_after_the_grace() {
    let policy = drive_policy_for(Role::Ide, QUEUE_WORK);
    for kind in KINDS {
        assert!(
            may_drive(kind, STRANDED_GRACE_SECS, policy),
            "{kind:?} unclaimed for the grace must fall back to the ide"
        );
    }
}

/// `OXY_IDE_DEFER_AIRWAY` keeps its meaning: airway and only airway is left
/// for the fleet, and an airway run nobody took for the grace is driven by the
/// ide after all — the fallback the periodic tick used to supply on its own,
/// now in the predicate so every loop honours it. The dev environment sets
/// this one, so it must not quietly widen to other kinds.
#[test]
fn the_airway_gate_alone_still_defers_airway_and_nothing_else() {
    let policy = drive_policy_for(Role::Ide, AIRWAY);
    for kind in KINDS {
        let is_airway = kind == Some(AIRWAY_SOURCE_TYPE);
        assert_eq!(may_drive(kind, FRESH, policy), !is_airway, "{kind:?}");
        assert!(
            may_drive(kind, STRANDED_GRACE_SECS, policy),
            "{kind:?}: unclaimed for the grace, the airway gate must let the ide take it"
        );
    }
}

/// Both set: the broader gate wins. If the narrower one won, setting the new
/// flag on the one environment that already had the old one would do nothing.
#[test]
fn the_broader_gate_wins_when_both_are_set() {
    assert_eq!(
        drive_policy_for(Role::Ide, BOTH),
        drive_policy_for(Role::Ide, QUEUE_WORK)
    );
    assert!(!may_drive(
        Some("preagg_cycle"),
        FRESH,
        drive_policy_for(Role::Ide, BOTH)
    ));
}

/// `All` is the single-process deployment: the ide IS the fleet, so deferring
/// would hand the run to nobody and make every job wait out the grace. It must
/// never defer, whichever gate is set — which is why the arms key on
/// `Role::Ide` and not on `process_can_compile()`, true for both.
#[test]
fn a_single_process_deployment_never_defers() {
    for defer in EVERY_GATE {
        let policy = drive_policy_for(Role::All, defer);
        for kind in KINDS {
            assert!(
                may_drive(kind, FRESH, policy),
                "Role::All with {defer:?} left {kind:?} for a fleet that does not exist"
            );
        }
    }
}

/// The pre-existing compile gate survives both ide gates. A worker (and a
/// `serve` whose driver was forced on) owns no working copy, so it declines
/// compile at any age — and must never decline anything else, since it is the
/// node all deferred work is being handed to.
#[test]
fn a_worker_declines_compile_forever_and_takes_everything_else() {
    for role in [Role::Worker, Role::Serve] {
        for defer in EVERY_GATE {
            let policy = drive_policy_for(role, defer);
            for kind in KINDS {
                let is_compile = kind == Some(COMPILE_SOURCE_TYPE);
                for age in [FRESH, STRANDED_GRACE_SECS, 24 * 60 * 60] {
                    assert_eq!(
                        may_drive(kind, age, policy),
                        !is_compile,
                        "{role:?} with {defer:?}: {kind:?} at {age}s"
                    );
                }
            }
        }
    }
}

/// The handoff is complete in both directions. Under every gate combination,
/// each kind has a role that drives it the moment it is queued: what the ide
/// leaves, a worker takes; what a worker cannot run, the ide keeps. A kind
/// neither took would only ever move on the grace fallback.
#[test]
fn no_kind_falls_between_the_ide_and_the_workers() {
    for defer in EVERY_GATE {
        let ide = drive_policy_for(Role::Ide, defer);
        let worker = drive_policy_for(Role::Worker, defer);
        for kind in KINDS {
            assert!(
                may_drive(kind, FRESH, ide) || may_drive(kind, FRESH, worker),
                "{kind:?} with {defer:?}: neither the ide nor a worker drives it at selection"
            );
        }
    }
}

/// Everything the deferring ide keeps is something no worker will take —
/// otherwise the list is a preference dressed as a requirement. And the
/// converse: everything a worker refuses, the deferring ide keeps.
#[test]
fn the_ide_keeps_exactly_what_the_workers_cannot_run() {
    let worker = drive_policy_for(Role::Worker, QUEUE_WORK);
    for kind in FACTORY_ONLY_KINDS {
        assert!(
            !may_drive(Some(kind), FRESH, worker),
            "{kind} is kept on the ide but a worker would drive it too"
        );
    }
    let ide = drive_policy_for(Role::Ide, QUEUE_WORK);
    for kind in KINDS {
        if !may_drive(kind, FRESH, worker) {
            assert!(
                may_drive(kind, FRESH, ide),
                "{kind:?} is refused by workers and left by the deferring ide"
            );
        }
    }
}

fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |name| {
        pairs
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    }
}

/// Both gates default off — the reader over an empty environment.
#[test]
fn both_gates_are_off_when_nothing_is_set() {
    assert_eq!(IdeDeferral::read(env(&[])), NEITHER);
}

/// Each variable switches its own gate and not the other. Spelled as literals
/// on purpose: these names are an operator-facing contract (charts set them),
/// so a rename must fail a test rather than follow the constant.
#[test]
fn each_env_var_switches_only_its_own_gate() {
    assert_eq!(IDE_DEFER_AIRWAY_ENV, "OXY_IDE_DEFER_AIRWAY");
    assert_eq!(IDE_DEFER_QUEUE_WORK_ENV, "OXY_IDE_DEFER_QUEUE_WORK");
    assert_eq!(
        IdeDeferral::read(env(&[("OXY_IDE_DEFER_AIRWAY", "1")])),
        AIRWAY
    );
    assert_eq!(
        IdeDeferral::read(env(&[("OXY_IDE_DEFER_QUEUE_WORK", "1")])),
        QUEUE_WORK
    );
    assert_eq!(
        IdeDeferral::read(env(&[
            ("OXY_IDE_DEFER_AIRWAY", "1"),
            ("OXY_IDE_DEFER_QUEUE_WORK", "1"),
        ])),
        BOTH
    );
}

/// The new gate parses exactly as the old one does: four truthy spellings,
/// everything else off — including an empty value and a different case, so a
/// chart that templates `"false"` or leaves the value blank stays off.
#[test]
fn the_new_gate_parses_like_the_old_one() {
    fn read_one(name: &'static str, value: &'static str) -> IdeDeferral {
        IdeDeferral::read(move |n| (n == name).then(|| value.to_string()))
    }
    for v in ["1", "true", "yes", "on"] {
        assert!(read_one("OXY_IDE_DEFER_QUEUE_WORK", v).queue_work, "{v:?}");
        assert!(read_one("OXY_IDE_DEFER_AIRWAY", v).airway, "{v:?}");
    }
    for v in ["", "0", "false", "no", "off", "TRUE", "2", " 1"] {
        assert!(!read_one("OXY_IDE_DEFER_QUEUE_WORK", v).queue_work, "{v:?}");
        assert!(!read_one("OXY_IDE_DEFER_AIRWAY", v).airway, "{v:?}");
    }
}

/// The real reader, against the ambient environment — pins that neither gate
/// is on unless something set it. Neither variable is set in CI.
#[test]
fn deferral_is_off_unless_an_env_var_is_set() {
    assert_eq!(
        IdeDeferral::from_env(),
        NEITHER,
        "OXY_IDE_DEFER_AIRWAY and OXY_IDE_DEFER_QUEUE_WORK must default off"
    );
}
