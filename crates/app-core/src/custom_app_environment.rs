//! The named environments of a custom app: `production`, `staging`, and one
//! `dev-<handle>` slot per engineer.
//!
//! Pure naming rules, shared by host parsing (`custom_apps_host_dispatch`) and the
//! `app_environments` writes in `oxy-app`. The database carries the same rules as a
//! CHECK constraint (`m20260922_000001_app_environments`); a DB test pins the two
//! together, so change both or neither.

use std::fmt;

/// Longest dev-slot handle. `dev-` + 12 + `--` + `--` leaves 43 bytes of a 63-byte
/// DNS label for `<org>` + `<slug>` (spec §3.2).
pub const DEV_HANDLE_MAX_LEN: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AppEnvironment {
    Production,
    Staging,
    Dev { handle: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEnvironmentKind {
    Production,
    Staging,
    Dev,
}

impl AppEnvironmentKind {
    /// The `app_environments.kind` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Staging => "staging",
            Self::Dev => "dev",
        }
    }
}

impl AppEnvironment {
    /// Parse an `app_environments.name`. `None` for anything that is not exactly a
    /// valid name. There is no normalisation: `Production` is not `production`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "production" => Some(Self::Production),
            "staging" => Some(Self::Staging),
            _ => {
                let handle = name.strip_prefix("dev-")?;
                is_valid_dev_handle(handle).then(|| Self::Dev {
                    handle: handle.to_string(),
                })
            }
        }
    }

    /// The `app_environments.name` value.
    pub fn name(&self) -> String {
        match self {
            Self::Production => "production".to_string(),
            Self::Staging => "staging".to_string(),
            Self::Dev { handle } => format!("dev-{handle}"),
        }
    }

    pub fn kind(&self) -> AppEnvironmentKind {
        match self {
            Self::Production => AppEnvironmentKind::Production,
            Self::Staging => AppEnvironmentKind::Staging,
            Self::Dev { .. } => AppEnvironmentKind::Dev,
        }
    }
}

impl fmt::Display for AppEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// Lowercase ASCII letters, digits and single hyphens; 1..=12 characters; no
/// leading or trailing hyphen. `--` is the host delimiter, so it can never appear.
pub fn is_valid_dev_handle(handle: &str) -> bool {
    !handle.is_empty()
        && handle.len() <= DEV_HANDLE_MAX_LEN
        && handle
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !handle.starts_with('-')
        && !handle.ends_with('-')
        && !handle.contains("--")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_two_fixed_environments() {
        assert_eq!(
            AppEnvironment::parse("production"),
            Some(AppEnvironment::Production)
        );
        assert_eq!(
            AppEnvironment::parse("staging"),
            Some(AppEnvironment::Staging)
        );
    }

    #[test]
    fn parses_a_dev_slot() {
        assert_eq!(
            AppEnvironment::parse("dev-luong"),
            Some(AppEnvironment::Dev {
                handle: "luong".into()
            })
        );
        assert_eq!(
            AppEnvironment::parse("dev-a1-b2"),
            Some(AppEnvironment::Dev {
                handle: "a1-b2".into()
            })
        );
    }

    #[test]
    fn rejects_malformed_names() {
        for name in [
            "",
            "prod",
            "Production",
            "dev",
            "dev-",
            "dev--x",
            "dev-x-",
            "dev-UPPER",
            "dev-a--b",
            "dev-a_b",
            "dev-abcdefghijklm",
        ] {
            assert_eq!(AppEnvironment::parse(name), None, "{name:?} must not parse");
        }
    }

    #[test]
    fn a_12_character_handle_is_the_longest_allowed() {
        let handle = "abcdefghijkl";
        assert_eq!(handle.len(), DEV_HANDLE_MAX_LEN);
        assert!(AppEnvironment::parse(&format!("dev-{handle}")).is_some());
    }

    #[test]
    fn name_round_trips_through_parse() {
        for env in [
            AppEnvironment::Production,
            AppEnvironment::Staging,
            AppEnvironment::Dev {
                handle: "luong".into(),
            },
        ] {
            assert_eq!(AppEnvironment::parse(&env.name()), Some(env.clone()));
        }
    }

    #[test]
    fn kind_names_match_the_database_check() {
        assert_eq!(AppEnvironment::Production.kind().as_str(), "production");
        assert_eq!(AppEnvironment::Staging.kind().as_str(), "staging");
        assert_eq!(
            AppEnvironment::Dev { handle: "x".into() }.kind().as_str(),
            "dev"
        );
    }
}
