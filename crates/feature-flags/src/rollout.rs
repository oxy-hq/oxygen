//! Rollout gates: registry flags of kind [`FlagKind::Rollout`], each with a
//! `sunset`, and the deprecated environment override that carries a
//! deployment from "set the variable" to "flip the flag" without a behaviour
//! change at merge.
//!
//! Two compile-boundary gates started life as environment variables read at
//! their call sites (`OXY_COMPILE_REFUSE_FILE_CREDENTIALS`,
//! `OXY_COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS`). They are now flags, so an
//! operator turns one on from the admin page or `oxyc flags set <flag> --on`,
//! per deployment, with the kill switch and the audit trail every flag has, and
//! so the registry's sunset test retires them on time. For **one release** the
//! variable still wins when it is set, with a deprecation warning logged once
//! per process, so a deployment that sets it today behaves exactly as it did;
//! unset, the flag decides. Both variables took `1` only, and still do.
//!
//! A third, `compile_reconcile` (and its `OXY_COMPILE_RECONCILE` override), was
//! retired: with no working copy left, a commit compile from git is the only way a
//! workspace compiles, so the branch check and the push webhook always run.
//!
//! The flag side reads the in-process cache (`super::cache`), which is loaded at
//! boot by every process that runs a gated path — `oxy serve` and `oxy worker`
//! (`init`, with the refresh) and the one-shot `oxy compile` (`load_once`). A
//! process that skipped that load would read every gate at its registry default,
//! off, whatever the admin page says; `oxy-workspace-runtime`'s
//! `tests/integration/rollout_gates.rs` guards the three boot paths.
//!
//! Retiring a gate: `internal-docs/feature-flags.md`, "Rollout gates and their
//! sunset".
//!
//! [`FlagKind::Rollout`]: super::registry::FlagKind::Rollout

use std::collections::BTreeSet;
use std::sync::{LazyLock, Mutex};

use super::cache;

/// Registry key of the gate that fails (rather than warns on) a compile whose
/// config reads a key file on a cloud role.
pub const COMPILE_REFUSE_FILE_CREDENTIALS_FLAG: &str = "compile_refuse_file_credentials";
/// Registry key of the gate that fails (rather than warns on) an automation
/// carrying a retired step.
pub const COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS_FLAG: &str =
    "compile_refuse_retired_automation_steps";

/// Registry key of the gate that fails (rather than warns on) a compile that
/// finds a classic `*.agent.yml` file, or an automation `agent_ref` naming one
/// (`2026-11-09-classic-agent-retirement.md`, phase 2). Born as a flag, so it
/// has no environment override and no [`RolloutGate`]; read with
/// `cache::is_enabled`.
pub const COMPILE_REFUSE_CLASSIC_AGENTS_FLAG: &str = "compile_refuse_classic_agents";

/// The deprecated overrides. Read only by [`is_on`], and inventoried in
/// `internal-docs/configuration.md` as rollout gates.
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

fn is_one(value: &str) -> bool {
    value == "1"
}

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
        assert!(is_on_with(
            &COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS,
            unset,
            |_| true
        ));
        assert!(!is_on_with(
            &COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS,
            unset,
            |_| false
        ));
        assert!(is_on_with(&COMPILE_REFUSE_FILE_CREDENTIALS, unset, |key| {
            key == COMPILE_REFUSE_FILE_CREDENTIALS_FLAG
        }));
    }

    #[test]
    fn set_the_variable_wins_in_both_directions() {
        let set = |v: &'static str| move |_: &str| Some(v.to_string());
        for gate in [
            &COMPILE_REFUSE_FILE_CREDENTIALS,
            &COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS,
        ] {
            assert!(is_on_with(gate, set("1"), |_| false), "{}", gate.env);
            assert!(!is_on_with(gate, set("0"), |_| true), "{}", gate.env);
        }
    }

    /// Both variables took `1` only: `true` is not on.
    #[test]
    fn each_gate_keeps_the_truthiness_its_variable_had() {
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
    fn compile_refuse_classic_agents_is_a_registered_rollout_flag_that_starts_off() {
        let def = super::super::registry::get(COMPILE_REFUSE_CLASSIC_AGENTS_FLAG)
            .expect("compile_refuse_classic_agents is registered");
        assert_eq!(def.kind, super::super::registry::FlagKind::Rollout);
        assert!(!def.default_enabled, "the refusal ships off");
        assert_eq!(def.sunset, Some("2027-03-31"));
    }
}
