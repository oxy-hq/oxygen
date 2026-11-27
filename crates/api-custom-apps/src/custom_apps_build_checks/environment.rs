//! Where an attempt of a build's checks runs: the build on **staging** (every
//! build that reaches it), or the build **production** serves after a move.
//! Never a sandbox: a sandbox's checks are `oxyc checks run`'s own.
//!
//! Every route that names an attempt takes it: `"environment"` in a runner
//! route's body, `?environment=` in the console's reads and a file's query.
//! Absent is staging, so a request written before production attempts existed
//! reads as it always did.

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::error::ChecksError;
use super::kind::CheckKind;
use super::settings;

/// The environment of an attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CheckEnvironment {
    #[default]
    Staging,
    Production,
}

impl CheckEnvironment {
    /// The stored `environment` of an attempt's rows: a wire contract.
    pub fn name(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }

    /// The environment a request names; staging when it names none. `400
    /// invalid_environment` for any other name (a sandbox's included).
    pub fn named(raw: Option<&str>) -> Result<Self, ChecksError> {
        match raw.map(str::trim) {
            None | Some("") | Some("staging") => Ok(Self::Staging),
            Some("production") => Ok(Self::Production),
            Some(other) => Err(ChecksError::bad_request(
                "invalid_environment",
                format!(
                    "`{}` is not an environment a build's checks run in: `staging` or `production`",
                    super::kind::cap(other, 40)
                ),
            )),
        }
    }

    /// The environment a stored attempt names; `None` for a row this build
    /// cannot place.
    pub fn stored(raw: &str) -> Option<Self> {
        [Self::Staging, Self::Production]
            .into_iter()
            .find(|environment| environment.name() == raw)
    }

    /// The app environment the attempt's pages are served by.
    pub fn app_environment(self) -> AppEnvironment {
        match self {
            Self::Staging => AppEnvironment::Staging,
            Self::Production => AppEnvironment::Production,
        }
    }

    /// The kinds an attempt here is queued with: staging's four, or six where
    /// the org's promote checks gate is on (`gated`); production's one.
    pub fn kinds(self, gated: bool) -> Vec<CheckKind> {
        match self {
            Self::Staging => CheckKind::of_staging_attempt(gated),
            Self::Production => CheckKind::PRODUCTION.to_vec(),
        }
    }
}

/// The kinds an attempt in the stored `environment` of `org_id` is queued
/// with, the gate read now (`settings::enabled_for`). An environment this
/// build cannot place is queued as staging's.
pub fn kinds_of(environment: &str, org_id: uuid::Uuid) -> Vec<CheckKind> {
    CheckEnvironment::stored(environment)
        .unwrap_or_default()
        .kinds(settings::enabled_for(org_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_that_names_none_reads_staging_and_a_sandbox_is_refused() {
        for staging in [None, Some(""), Some("staging"), Some(" staging ")] {
            assert_eq!(
                CheckEnvironment::named(staging).unwrap(),
                CheckEnvironment::Staging,
                "{staging:?}"
            );
        }
        assert_eq!(
            CheckEnvironment::named(Some("production")).unwrap(),
            CheckEnvironment::Production
        );
        for refused in ["dev-a1", "prod", "Production", "preview"] {
            let e = CheckEnvironment::named(Some(refused)).unwrap_err();
            assert_eq!(e.code(), Some("invalid_environment"), "{refused}");
        }
    }

    #[test]
    fn each_environment_has_its_stored_name_its_app_environment_and_its_kinds() {
        let (staging, production) = (CheckEnvironment::Staging, CheckEnvironment::Production);
        assert_eq!(staging.name(), AppEnvironment::Staging.name());
        assert_eq!(production.name(), AppEnvironment::Production.name());
        assert_eq!(CheckEnvironment::stored("production"), Some(production));
        assert_eq!(CheckEnvironment::stored("dev-a1"), None);
        assert_eq!(production.app_environment(), AppEnvironment::Production);
        assert_eq!(staging.kinds(false), CheckKind::PHASE_ONE.to_vec());
        assert_eq!(staging.kinds(true).len(), 6);
        // The gate adds nothing to a production attempt.
        for gated in [false, true] {
            assert_eq!(production.kinds(gated), [CheckKind::ProductionSmoke]);
        }
    }
}
