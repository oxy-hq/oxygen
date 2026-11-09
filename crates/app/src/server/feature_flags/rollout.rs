//! Rollout gates: registry flags of kind [`FlagKind::Rollout`], each with a
//! `sunset`, and the deprecated environment override that carries a
//! deployment from "set the variable" to "flip the flag" without a behaviour
//! change at merge.
//!
//! Three compile-boundary gates started life as environment variables read at
//! their call sites (`OXY_COMPILE_RECONCILE`, `OXY_COMPILE_REFUSE_FILE_CREDENTIALS`,
//! `OXY_COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS`). They are now flags, so an
//! operator turns one on from the admin page or `oxyc flags set <flag> --on`,
//! per deployment, with the kill switch and the audit trail every flag has, and
//! so the registry's sunset test retires them on time. For **one release** the
//! variable still wins when it is set, with a deprecation warning logged once
//! per process, so a deployment that sets it today behaves exactly as it did;
//! unset, the flag decides. The variable's own truthiness is kept per gate
//! (`OXY_COMPILE_RECONCILE` took `1`/`true`/`yes`/`on`; the other two took `1`
//! only), for the same reason.
//!
//! The flag side reads the in-process cache (`super::cache`), which is loaded at
//! boot by every process that runs a gated path — `oxy serve` and `oxy worker`
//! (`init`, with the refresh) and the one-shot `oxy compile` (`load_once`). A
//! process that skipped that load would read every gate at its registry default,
//! off, whatever the admin page says; `tests/platform/rollout_gates.rs` guards
//! the three boot paths.
//!
//! `ide_retired` is a rollout gate born as a flag: it has no variable to carry
//! over, so it has no [`RolloutGate`] and is read with `cache::is_enabled`.
//!
//! Retiring a gate: `internal-docs/feature-flags.md`, "Rollout gates and their
//! sunset".
//!
//! [`FlagKind::Rollout`]: super::registry::FlagKind::Rollout

use std::collections::BTreeSet;
use std::sync::{LazyLock, Mutex};

use super::cache;

/// Registry key of the gate on the periodic branch check and the GitHub push
/// webhook that queue a commit compile when a default branch moves.
pub const COMPILE_RECONCILE_FLAG: &str = "compile_reconcile";
/// Registry key of the gate that fails (rather than warns on) a compile whose
/// config reads a key file on a cloud role.
pub const COMPILE_REFUSE_FILE_CREDENTIALS_FLAG: &str = "compile_refuse_file_credentials";
/// Registry key of the gate that fails (rather than warns on) an automation
/// carrying a retired step.
pub const COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS_FLAG: &str =
    "compile_refuse_retired_automation_steps";

/// Registry key of the gate that retires the in-product IDE on a deployment: the
/// web app hides its file, database and git entry points and shows where the
/// repository is instead, and the server refuses the IDE's routes with `410 Gone`
/// (`server::ide_retired`). A new gate, so it has no environment override.
pub const IDE_RETIRED_FLAG: &str = "ide_retired";

/// The deprecated overrides. Read only by [`is_on`], and inventoried in
/// `internal-docs/configuration.md` as rollout gates.
pub const COMPILE_RECONCILE_ENV: &str = "OXY_COMPILE_RECONCILE";
pub const COMPILE_REFUSE_FILE_CREDENTIALS_ENV: &str = "OXY_COMPILE_REFUSE_FILE_CREDENTIALS";
pub const COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS_ENV: &str =
    "OXY_COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS";

/// A rollout flag and the environment variable that overrides it for one release.
pub struct RolloutGate {
    pub flag: &'static str,
    pub env: &'static str,
    /// Whether a set value of the variable turns the gate on: the truthiness
    /// the variable always had, so setting it today means what it meant.
    pub env_is_on: fn(&str) -> bool,
}

fn truthy(value: &str) -> bool {
    matches!(value, "1" | "true" | "yes" | "on")
}

fn is_one(value: &str) -> bool {
    value == "1"
}

pub const COMPILE_RECONCILE: RolloutGate = RolloutGate {
    flag: COMPILE_RECONCILE_FLAG,
    env: COMPILE_RECONCILE_ENV,
    env_is_on: truthy,
};

pub const COMPILE_REFUSE_FILE_CREDENTIALS: RolloutGate = RolloutGate {
    flag: COMPILE_REFUSE_FILE_CREDENTIALS_FLAG,
    env: COMPILE_REFUSE_FILE_CREDENTIALS_ENV,
    env_is_on: is_one,
};

pub const COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS: RolloutGate = RolloutGate {
    flag: COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS_FLAG,
    env: COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS_ENV,
    env_is_on: is_one,
};

/// Whether `gate` is on for this process: the variable when it is set (and a
/// deprecation warning, once per gate per process), the flag otherwise.
pub fn is_on(gate: &RolloutGate) -> bool {
    is_on_with(gate, |name| std::env::var(name).ok(), cache::is_enabled)
}

/// [`is_on`] over injected lookups, so a test exercises the precedence without
/// touching the process environment or the flag cache.
pub fn is_on_with(
    gate: &RolloutGate,
    var: impl Fn(&str) -> Option<String>,
    flag: impl Fn(&'static str) -> bool,
) -> bool {
    match var(gate.env) {
        Some(value) => {
            warn_once(gate, &value);
            (gate.env_is_on)(&value)
        }
        None => flag(gate.flag),
    }
}

/// Flags whose override has already been warned about in this process.
static WARNED: LazyLock<Mutex<BTreeSet<&'static str>>> = LazyLock::new(Default::default);

fn warn_once(gate: &RolloutGate, value: &str) {
    let mut warned = WARNED.lock().unwrap_or_else(|p| p.into_inner());
    if warned.insert(gate.flag) {
        tracing::warn!(
            target: "feature_flags",
            env = gate.env,
            flag = gate.flag,
            value,
            "{} is set and overrides the `{}` feature flag; the override is removed in the \
             next release. Set the flag instead (`oxyc flags set {} --on`) and unset the variable",
            gate.env,
            gate.flag,
            gate.flag
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unset(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn unset_the_flag_decides() {
        assert!(is_on_with(&COMPILE_RECONCILE, unset, |_| true));
        assert!(!is_on_with(&COMPILE_RECONCILE, unset, |_| false));
        assert!(is_on_with(&COMPILE_REFUSE_FILE_CREDENTIALS, unset, |key| {
            key == COMPILE_REFUSE_FILE_CREDENTIALS_FLAG
        }));
    }

    #[test]
    fn set_the_variable_wins_in_both_directions() {
        let set = |v: &'static str| move |_: &str| Some(v.to_string());
        assert!(is_on_with(&COMPILE_RECONCILE, set("1"), |_| false));
        assert!(!is_on_with(&COMPILE_RECONCILE, set("0"), |_| true));
    }

    /// `OXY_COMPILE_RECONCILE` always took four spellings; the other two took `1`.
    #[test]
    fn each_gate_keeps_the_truthiness_its_variable_had() {
        for on in ["1", "true", "yes", "on"] {
            assert!(
                is_on_with(&COMPILE_RECONCILE, |_| Some(on.into()), |_| false),
                "{on}"
            );
        }
        for off in ["", "0", "false", "no", "TRUE"] {
            assert!(
                !is_on_with(&COMPILE_RECONCILE, |_| Some(off.into()), |_| true),
                "{off:?}"
            );
        }
        for gate in [
            &COMPILE_REFUSE_FILE_CREDENTIALS,
            &COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS,
        ] {
            assert!(is_on_with(gate, |_| Some("1".into()), |_| false));
            assert!(
                !is_on_with(gate, |_| Some("true".into()), |_| true),
                "{}",
                gate.env
            );
        }
    }

    #[test]
    fn every_gate_names_a_registered_rollout_flag() {
        for gate in [
            &COMPILE_RECONCILE,
            &COMPILE_REFUSE_FILE_CREDENTIALS,
            &COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS,
        ] {
            let def = super::super::registry::get(gate.flag)
                .unwrap_or_else(|| panic!("{} is not in the registry", gate.flag));
            assert_eq!(
                def.kind,
                super::super::registry::FlagKind::Rollout,
                "{}",
                gate.flag
            );
            assert!(gate.env.starts_with("OXY_"), "{}", gate.env);
        }
    }

    #[test]
    fn ide_retired_is_a_registered_rollout_flag_that_starts_off() {
        let def = super::super::registry::get(IDE_RETIRED_FLAG).expect("ide_retired is registered");
        assert_eq!(def.kind, super::super::registry::FlagKind::Rollout);
        assert!(
            !def.default_enabled,
            "the IDE stays on until a deployment turns it off"
        );
        assert!(def.sunset.is_some());
    }
}
