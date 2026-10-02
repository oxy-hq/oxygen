//! The named environments of a custom app: `production`, `staging`, and any
//! number of `dev-<handle>` sandboxes (`internal-docs/custom-app-sandboxes.md`).
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

    /// The label of this environment's sibling Airhouse schema
    /// (`airhouse::app_schema::environment_schema`): `None` for production,
    /// which writes the app's own schema, `staging`, or `dev_<handle>` with
    /// each `-` as `_`. A handle holds no underscore, so two sandboxes never
    /// share a label.
    pub fn schema_label(&self) -> Option<String> {
        match self {
            Self::Production => None,
            Self::Staging => Some("staging".to_string()),
            Self::Dev { handle } => Some(format!("dev_{}", handle.replace('-', "_"))),
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

    /// The label of an environment's sibling Airhouse schema: production has
    /// none, staging's is its name, and a sandbox's is its name with every
    /// hyphen as an underscore (a schema name cannot hold a hyphen unquoted).
    #[test]
    fn schema_label_for_each_kind() {
        assert_eq!(AppEnvironment::Production.schema_label(), None);
        assert_eq!(
            AppEnvironment::Staging.schema_label().as_deref(),
            Some("staging")
        );
        assert_eq!(
            AppEnvironment::Dev {
                handle: "luong".into()
            }
            .schema_label()
            .as_deref(),
            Some("dev_luong")
        );
    }

    #[test]
    fn schema_label_maps_each_hyphen_of_a_handle_to_one_underscore() {
        assert_eq!(
            AppEnvironment::Dev {
                handle: "a1-b2-c3".into()
            }
            .schema_label()
            .as_deref(),
            Some("dev_a1_b2_c3")
        );
    }

    /// Two different sandboxes never share a label: a handle holds no
    /// underscore, so the hyphen-to-underscore map loses nothing.
    #[test]
    fn schema_labels_of_distinct_handles_are_distinct() {
        let labels: std::collections::HashSet<String> = ["a-b", "ab", "a", "b", "a-b-c", "a-bc"]
            .iter()
            .map(|h| {
                AppEnvironment::Dev {
                    handle: (*h).to_string(),
                }
                .schema_label()
                .expect("a sandbox has a label")
            })
            .collect();
        assert_eq!(labels.len(), 6);
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
