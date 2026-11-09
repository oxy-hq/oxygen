//! `OXY_ROLE`, as the deployment set it.
//!
//! The role is the one variable whose *default* depends on the command: `oxy
//! serve` is `all` unless told otherwise, `oxy worker` is `worker` (running the
//! worker binary is stronger evidence than a chart that never set the variable).
//! So this module parses the setting and leaves the default to
//! `crate::process_role`, which knows the command.

use oxy_shared::fleet_role::Role;

use super::ConfigError;

/// What `OXY_ROLE` says, before a command applies its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleSetting {
    /// Not set: the command's default applies.
    Unset,
    /// One of `ide`, `serve`, `worker`, `all`.
    Set(Role),
    /// Set to something that is not a role. Kept verbatim so a strict boot can
    /// name it and a lenient one can warn and fall back to the command's default,
    /// as every read before this module did.
    Unrecognised(String),
}

impl RoleSetting {
    /// Exact, case-sensitive, untrimmed: the spellings the charts have always
    /// used, and nothing a typo could pass as.
    pub fn parse(raw: Option<String>) -> Self {
        match raw.as_deref() {
            None => RoleSetting::Unset,
            Some("ide") => RoleSetting::Set(Role::Ide),
            Some("serve") => RoleSetting::Set(Role::Serve),
            Some("worker") => RoleSetting::Set(Role::Worker),
            Some("all") => RoleSetting::Set(Role::All),
            Some(other) => RoleSetting::Unrecognised(other.to_string()),
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        match self {
            RoleSetting::Unrecognised(value) => Err(ConfigError {
                var: "OXY_ROLE",
                value: value.clone(),
                problem: "expected one of ide, serve, worker, all".to_string(),
            }),
            _ => Ok(()),
        }
    }

    /// The role, with `default` standing in for an unset variable. An
    /// unrecognised value also takes the default, after one warning: the
    /// lenient path, for a process that did not go through `BootConfig::init`.
    pub fn resolve(&self, default: Role) -> Role {
        match self {
            RoleSetting::Set(role) => *role,
            RoleSetting::Unset => default,
            RoleSetting::Unrecognised(other) => {
                tracing::warn!(
                    value = other.as_str(),
                    default = default.as_str(),
                    "OXY_ROLE: unrecognised value; using this command's default"
                );
                default
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_roles_parse_and_nothing_else_does() {
        assert_eq!(RoleSetting::parse(None), RoleSetting::Unset);
        for (raw, role) in [
            ("ide", Role::Ide),
            ("serve", Role::Serve),
            ("worker", Role::Worker),
            ("all", Role::All),
        ] {
            assert_eq!(RoleSetting::parse(Some(raw.into())), RoleSetting::Set(role));
        }
        for junk in ["IDE", " ide", "compile", ""] {
            assert_eq!(
                RoleSetting::parse(Some(junk.into())),
                RoleSetting::Unrecognised(junk.to_string()),
                "{junk:?}"
            );
        }
    }

    #[test]
    fn resolve_applies_the_default_to_unset_and_unrecognised() {
        assert_eq!(RoleSetting::Unset.resolve(Role::Worker), Role::Worker);
        assert_eq!(RoleSetting::Set(Role::Ide).resolve(Role::Worker), Role::Ide);
        assert_eq!(
            RoleSetting::Unrecognised("compile".into()).resolve(Role::All),
            Role::All
        );
    }

    #[test]
    fn only_an_unrecognised_value_fails_validation() {
        assert_eq!(RoleSetting::Unset.validate(), Ok(()));
        assert_eq!(RoleSetting::Set(Role::Serve).validate(), Ok(()));
        let err = RoleSetting::Unrecognised("compile".into())
            .validate()
            .unwrap_err();
        assert_eq!(err.var, "OXY_ROLE");
    }
}
