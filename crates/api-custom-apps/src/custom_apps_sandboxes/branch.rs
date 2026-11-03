//! What a create records of where a sandbox was cut from and how its planes
//! behave: the parent (`from`), the fork point (`at`) and the planes it chose
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5).
//!
//! A create that names `from` also starts the sandbox on the build its parent
//! serves and queues that build's migrations for it (`copy_build`). One that
//! names none of them is what it always was.

use chrono::{DateTime, FixedOffset, Utc};
use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::Value;
use uuid::Uuid;

use super::SandboxError;

/// The branch facts a create stores beside the sandbox's name. The default
/// leaves every column at its own default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewBranch {
    /// The environment named by `from`, checked against the app's rows by
    /// [`check_parent`] under the app's lock.
    pub from: Option<String>,
    /// `at`, already parsed by [`parse_fork_at`].
    pub fork_at: Option<DateTime<FixedOffset>>,
    /// The object to store (`RequestedPlanes::stored`); `None` is `{}`.
    pub planes: Option<Value>,
}

/// `at`, an RFC 3339 time that is not in the future: a fork point is a moment
/// that has happened.
pub fn parse_fork_at(at: &str, now: DateTime<Utc>) -> Result<DateTime<FixedOffset>, SandboxError> {
    let parsed = DateTime::parse_from_rfc3339(at).map_err(|e| {
        SandboxError::InvalidForkAt(format!("{at:?} is not an RFC 3339 time ({e})"))
    })?;
    if parsed.with_timezone(&Utc) > now {
        return Err(SandboxError::InvalidForkAt(format!(
            "{at:?} is in the future: a fork point is a moment that has happened"
        )));
    }
    Ok(parsed)
}

/// `from`, when it names an environment a sandbox may be cut from:
/// production, staging, or an active sandbox of this app, among `sandboxes`
/// (the app's sandbox rows, read under its lock). This checks that the parent
/// exists; whether the caller may open it is the handler's question
/// (`may_fork_from`), asked before the create. A sandbox agent token (`token`)
/// is also held to a sandbox it created here, so a colleague's is answered as
/// one that does not exist even if that check were skipped.
pub(super) fn check_parent(
    from: &str,
    sandboxes: &[app_environments::Model],
    token: Option<Uuid>,
) -> Result<String, SandboxError> {
    let invalid = || SandboxError::InvalidParent(from.to_string());
    match AppEnvironment::parse(from).ok_or_else(invalid)? {
        AppEnvironment::Production | AppEnvironment::Staging => Ok(from.to_string()),
        AppEnvironment::Dev { .. } => sandboxes
            .iter()
            .find(|row| row.name == from)
            .filter(|row| row.deleting_at.is_none())
            .filter(|row| token.is_none_or(|token| row.created_by_token_id == Some(token)))
            .map(|row| row.name.clone())
            .ok_or_else(invalid),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, hour, 0, 0).unwrap()
    }

    fn sandbox(name: &str, deleting: bool, token: Option<Uuid>) -> app_environments::Model {
        let now = at(9).fixed_offset();
        app_environments::Model {
            app_id: Uuid::from_u128(1),
            name: name.to_string(),
            kind: "dev".to_string(),
            owner_user_id: None,
            build_id: None,
            updated_by: None,
            updated_at: now,
            created_at: now,
            deleting_at: deleting.then_some(now),
            oltp_schema: None,
            created_by_token_id: token,
            schedules_enabled: false,
            parent_name: None,
            fork_at: None,
            planes: serde_json::json!({}),
            semantic_revision_id: None,
            semantic_git_sha: None,
        }
    }

    #[test]
    fn a_fork_point_is_a_past_rfc_3339_time() {
        let parsed = parse_fork_at("2026-11-02T08:30:00+01:00", at(12)).expect("past");
        assert_eq!(
            parsed.with_timezone(&Utc),
            at(7) + chrono::Duration::minutes(30)
        );
        parse_fork_at("2026-11-02T12:00:00Z", at(12)).expect("now is not the future");
        for bad in ["yesterday", "2026-11-02", "2026-11-02T12:00:01Z"] {
            let error = parse_fork_at(bad, at(12)).expect_err(bad);
            assert_eq!(error.code(), "invalid_fork_at", "{bad}");
        }
    }

    #[test]
    fn a_parent_is_production_staging_or_an_active_sandbox() {
        let rows = [
            sandbox("dev-a1", false, None),
            sandbox("dev-gone", true, None),
        ];
        for ok in ["production", "staging", "dev-a1"] {
            assert_eq!(check_parent(ok, &rows, None), Ok(ok.to_string()));
        }
        for bad in ["dev-gone", "dev-none", "nope", "", "DEV-a1"] {
            assert_eq!(
                check_parent(bad, &rows, None),
                Err(SandboxError::InvalidParent(bad.to_string())),
                "{bad}"
            );
        }
    }

    /// A sandbox agent token forks from a sandbox it created and from no
    /// colleague's; a person is not bound.
    #[test]
    fn a_token_forks_only_from_its_own_sandbox() {
        let (mine, theirs) = (Uuid::from_u128(7), Uuid::from_u128(8));
        let rows = [
            sandbox("dev-mine", false, Some(mine)),
            sandbox("dev-theirs", false, Some(theirs)),
            sandbox("dev-person", false, None),
        ];
        assert!(check_parent("dev-mine", &rows, Some(mine)).is_ok());
        for other in ["dev-theirs", "dev-person"] {
            assert!(check_parent(other, &rows, Some(mine)).is_err(), "{other}");
            assert!(check_parent(other, &rows, None).is_ok(), "{other}");
        }
    }
}
