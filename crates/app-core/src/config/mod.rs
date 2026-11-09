//! The typed boot configuration: every deployment `OXY_*` variable the app
//! layer reads, parsed once, with its default and its validation in one place.
//!
//! Before this module each read site did its own `std::env::var("OXY_…")`, with
//! the default encoded wherever the variable happened to be read (265 distinct
//! variables across the crates, 92 of them read directly in `oxy-app` alone).
//! The inventory, `internal-docs/configuration.md`, is generated from those
//! sites by `scripts/env-inventory.py`; the guard test
//! `crates/app/tests/routing/env_reads/` fails a new read outside this module.
//!
//! Two ways in, by design:
//!
//! * [`BootConfig::init`] — the server commands (`oxy serve`, `oxy worker`) call
//!   it first thing. It parses the environment **once**, refuses to boot on a
//!   value it cannot use (the error names the variable), and keeps the result
//!   for the process's life.
//! * [`BootConfig::current`] — what every read site calls. After `init` it is
//!   the stored config; before it (tests, `oxy run`, a CLI command that never
//!   boots a server) it is a fresh, lenient parse of the environment, so a test
//!   that sets a variable and calls the function under test still sees its
//!   value, exactly as the scattered reads behaved.
//!
//! Parsing never panics and never fails: a malformed value is kept as what it
//! is (an unrecognised role, an unparseable URL) so that [`BootConfig::validate`]
//! can name it and the lenient path can fall back to the old behaviour.

pub mod hosts;
pub mod role;

use std::sync::{Arc, OnceLock};

pub use hosts::HostsConfig;
pub use role::RoleSetting;

/// A value the deployment set that the process cannot use. The message names
/// the variable, its value and what was expected, so the fix is one line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{var}={value:?}: {problem}")]
pub struct ConfigError {
    pub var: &'static str,
    pub value: String,
    pub problem: String,
}

/// Everything below `oxy-app` reads about its deployment from the environment.
/// One field per concern; each knows its own variables and defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootConfig {
    /// `OXY_ROLE`: this process's fleet role, as the deployment set it.
    pub role: RoleSetting,
    /// The hosts and zones this deployment answers on.
    pub hosts: HostsConfig,
    /// `OXY_INTERNAL_ORGS`: the orgs Oxy operates for itself
    /// (`crate::internal_orgs`). Entries that are not UUIDs are skipped with a
    /// warning there, not here, so one typo never drops the list.
    pub internal_orgs: Vec<uuid::Uuid>,
}

static BOOT: OnceLock<Arc<BootConfig>> = OnceLock::new();

impl BootConfig {
    /// Parse a configuration from `var`, a lookup over variable names. Lenient:
    /// never fails, see [`BootConfig::validate`] for the strict half. Tests pass
    /// a closure over a map instead of mutating the process environment.
    pub fn parse(var: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            role: RoleSetting::parse(var("OXY_ROLE")),
            hosts: HostsConfig::parse(var),
            internal_orgs: crate::internal_orgs::parse_internal_orgs(
                var("OXY_INTERNAL_ORGS").as_deref(),
            ),
        }
    }

    /// Parse the process environment, leniently.
    pub fn from_env() -> Self {
        Self::parse(&|name| std::env::var(name).ok())
    }

    /// The first value the deployment set that this process cannot use, if any.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.role.validate()?;
        self.hosts.validate()
    }

    /// Parse the environment once for this process and keep it. Called first
    /// by the server commands; a value that fails [`BootConfig::validate`]
    /// refuses the boot with an error naming the variable. A second call
    /// returns what the first stored.
    pub fn init() -> Result<Arc<Self>, ConfigError> {
        if let Some(stored) = BOOT.get() {
            return Ok(Arc::clone(stored));
        }
        let parsed = Self::from_env();
        parsed.validate()?;
        Ok(Arc::clone(BOOT.get_or_init(|| Arc::new(parsed))))
    }

    /// This process's configuration: what [`BootConfig::init`] stored, or, in a
    /// process that never called it, a fresh lenient parse of the environment.
    pub fn current() -> Arc<Self> {
        match BOOT.get() {
            Some(stored) => Arc::clone(stored),
            None => Arc::new(Self::from_env()),
        }
    }

    /// Whether [`BootConfig::init`] has run in this process.
    pub fn is_initialised() -> bool {
        BOOT.get().is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use oxy_shared::fleet_role::Role;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn parse(pairs: &[(&str, &str)]) -> BootConfig {
        let map = env(pairs);
        BootConfig::parse(&|name| map.get(name).cloned())
    }

    #[test]
    fn an_empty_environment_parses_and_validates() {
        let cfg = parse(&[]);
        assert_eq!(cfg.role, RoleSetting::Unset);
        assert_eq!(cfg.hosts.api_url, None);
        assert!(cfg.internal_orgs.is_empty());
        assert_eq!(cfg.validate(), Ok(()));
    }

    #[test]
    fn a_full_environment_lands_in_typed_fields() {
        let cfg = parse(&[
            ("OXY_ROLE", "serve"),
            ("OXY_API_URL", "https://app-dev.oxygen-hq.com/api"),
            ("OXY_INTERNAL_ORGS", "0f8e2b9c-3a51-4d6e-9c1f-2b7a4e5d6c01"),
        ]);
        assert_eq!(cfg.role, RoleSetting::Set(Role::Serve));
        assert_eq!(
            cfg.hosts.admin_base_url().as_deref(),
            Some("https://app-dev.oxygen-hq.com")
        );
        assert_eq!(cfg.internal_orgs.len(), 1);
        assert_eq!(cfg.validate(), Ok(()));
    }

    #[test]
    fn validation_names_the_variable_and_the_value() {
        let err = parse(&[("OXY_ROLE", "compile")]).validate().unwrap_err();
        assert_eq!(err.var, "OXY_ROLE");
        assert_eq!(err.value, "compile");
        assert!(
            err.to_string().starts_with("OXY_ROLE=\"compile\": "),
            "{err}"
        );

        let err = parse(&[("OXY_API_URL", "not a url")])
            .validate()
            .unwrap_err();
        assert_eq!(err.var, "OXY_API_URL");
    }

    /// `current()` before `init()` reads the environment afresh, which is what
    /// lets the host-dispatch tests set a variable and call the function.
    #[test]
    fn current_before_init_is_a_fresh_parse() {
        assert!(
            !BootConfig::is_initialised(),
            "no test in this binary may call init()"
        );
        let a = BootConfig::current();
        let b = BootConfig::current();
        assert_eq!(*a, *b);
        assert!(
            !Arc::ptr_eq(&a, &b),
            "a fresh parse each time, not a cached one"
        );
    }
}
