//! An **environment's** semantic pin (`app_environments.semantic_revision_id`,
//! `semantic_git_sha`) and how it combines with a build's own.
//!
//! A build's pin (`--semantic-branch` at publish, `app_builds.
//! semantic_revision_id`, [`super::pinned_revision_for`]) is a fact about one
//! upload. The environment's pin is a fact about the environment: it follows a
//! semantic branch, so a request in it reads that branch's compiled model
//! whichever build the environment serves.
//!
//! **A plane whose home is not ready is refused, never routed elsewhere.**
//! The pin is [`EnvironmentPin::Ready`] while the pinned revision is a `ready`
//! revision of the app's workspace — served even when `semantic_git_sha` names
//! a newer commit still compiling, which keeps the environment answering from
//! the last good model. It is [`EnvironmentPin::NotReady`] when a commit is
//! recorded but no ready revision is pinned (the first compile, or one that
//! failed): the request is refused with [`PinNotReady`] rather than quietly
//! reading production's model. [`effective_pin`] is the one place the two pins
//! meet: a build's own pin wins.
//!
//! **Fails closed.** A lookup that errors, and a `planes` value that does not
//! parse, are [`EnvironmentPin::NotReady`] too (a retryable `503`), never
//! [`EnvironmentPin::Unset`]: reading production's or the build's model
//! because a database call blipped would be the fallback this module exists to
//! rule out, and function admission already refuses unparseable planes.
//!
//! Production never has an environment pin; callers skip the read for it.

use crate::planes::PlaneSpec;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

/// What an environment's semantic pin currently resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentPin {
    /// No environment pin: the environment's semantic plane is not `branch`
    /// (whatever the pin columns still hold), or the row is gone.
    Unset,
    /// A `ready` revision of the app's workspace to read.
    Ready(Uuid),
    /// The environment's semantic model cannot be served right now: its plane
    /// names a branch with no ready revision, or its planes (or the lookup)
    /// could not be read. Always refused.
    NotReady(PinNotReady),
}

/// Why an environment's semantic model cannot be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReadyCause {
    /// The plane names `branch`, which has no ready revision.
    Branch {
        branch: String,
        /// The commit the branch points at, once a pin has recorded one.
        /// `None` when the environment names a branch but no ready revision
        /// is pinned and no commit was recorded: the pin step after a planes
        /// write failed or has not run.
        git_sha: Option<String>,
    },
    /// The row's `planes` value does not parse, so what its semantic plane
    /// reads is unknown.
    PlanesUnreadable,
    /// The row could not be read (a database error). Retry.
    LookupFailed,
}

/// The environment's semantic model cannot be served: the request is refused
/// (`503 semantic_branch_not_ready`, retryable; `409` when the planes do not
/// parse), not served another model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.fix())]
pub struct PinNotReady {
    /// The environment that is refused (`staging`, `dev-<handle>`).
    pub environment: String,
    pub cause: NotReadyCause,
}

impl PinNotReady {
    /// The environment's plane names `branch`, with no ready revision pinned.
    pub fn branch(environment: &str, branch: String, git_sha: Option<String>) -> Self {
        Self {
            environment: environment.to_string(),
            cause: NotReadyCause::Branch { branch, git_sha },
        }
    }

    /// The environment's planes do not parse.
    pub fn planes_unreadable(environment: &str) -> Self {
        Self {
            environment: environment.to_string(),
            cause: NotReadyCause::PlanesUnreadable,
        }
    }

    /// The environment's row could not be read.
    pub fn lookup_failed(environment: &str) -> Self {
        Self {
            environment: environment.to_string(),
            cause: NotReadyCause::LookupFailed,
        }
    }

    /// Whether the refusal is planes that do not parse: not retryable, so it is
    /// answered `409`, not the `503` (`oxy_app_core::gate_response::pin_refused`).
    pub fn is_planes_unreadable(&self) -> bool {
        self.cause == NotReadyCause::PlanesUnreadable
    }

    /// The recorded commit, or `""` when none is (the shape the `503` body
    /// has always carried).
    pub fn git_sha(&self) -> &str {
        match &self.cause {
            NotReadyCause::Branch {
                git_sha: Some(sha), ..
            } => sha,
            _ => "",
        }
    }

    /// What the caller can do about it; it names the branch, and the commit
    /// when there is one.
    pub fn fix(&self) -> String {
        match &self.cause {
            NotReadyCause::Branch {
                branch,
                git_sha: Some(sha),
            } => format!(
                "this environment's semantic model is pinned to branch {branch:?}, whose head \
                 ({sha}) has not compiled yet (or failed to compile); retry shortly, push a fix, \
                 or run `oxyc env update <app> <env> --plane semantic=build-pin`"
            ),
            NotReadyCause::Branch {
                branch,
                git_sha: None,
            } => format!(
                "this environment's semantic model is set to branch {branch:?}, but no ready \
                 revision is pinned for it (the pin step after the planes write failed or has \
                 not run); send the same `oxyc env update <app> <env> --plane \
                 semantic=branch:{branch}` again to pin it, or run `oxyc env update <app> <env> \
                 --plane semantic=build-pin`"
            ),
            NotReadyCause::PlanesUnreadable => "this environment's planes could not be read (a \
                 value does not parse), so its semantic model is not served; set them again \
                 with `oxyc env update <app> <env> --plane semantic=build-pin`"
                .to_string(),
            NotReadyCause::LookupFailed => "this environment's semantic pin could not be read \
                 just now; retry shortly"
                .to_string(),
        }
    }
}

/// The pure half of [`environment_pin`], from the branch the environment's
/// semantic plane names (`Ok(None)` for any other mode, `Err` for planes that
/// do not parse).
///
/// **The mode decides whether the pin columns are read at all.** A mode that is
/// not `branch` ignores them, so a pin a failed clear left behind never serves
/// an old branch. In `branch` mode a ready pinned revision wins over a recorded
/// commit, a recorded commit with no ready revision is not ready, and *no pin
/// at all* is not ready either: the environment says it reads a branch, so it
/// is refused rather than served another model. (A planes write that changes
/// the branch clears the previous branch's pin first, so the revision read
/// here is always the named branch's.)
fn classify(
    environment: &str,
    semantic: Result<Option<String>, String>,
    ready_revision: Option<Uuid>,
    git_sha: Option<String>,
) -> EnvironmentPin {
    let branch = match semantic {
        Ok(Some(branch)) => branch,
        Ok(None) => return EnvironmentPin::Unset,
        Err(_) => return EnvironmentPin::NotReady(PinNotReady::planes_unreadable(environment)),
    };
    match (ready_revision, git_sha.filter(|sha| !sha.is_empty())) {
        (Some(revision), _) => EnvironmentPin::Ready(revision),
        (None, git_sha) => {
            EnvironmentPin::NotReady(PinNotReady::branch(environment, branch, git_sha))
        }
    }
}

/// The branch a `planes` object's `semantic` choice names (`None` for any other
/// mode). `Err` when the value does not parse: its semantic plane is unknown,
/// which [`classify`] refuses rather than reads as "no branch".
fn semantic_branch(planes: &serde_json::Value) -> Result<Option<String>, String> {
    PlaneSpec::parse(planes).map(|spec| spec.semantic_branch().map(str::to_string))
}

/// The pin to read: `build_pin` (the build's own) when it has one, else the
/// environment's. `Err` only when the build pins nothing and the environment's
/// branch is not ready.
pub fn effective_pin(
    build_pin: Option<Uuid>,
    env: &EnvironmentPin,
) -> Result<Option<Uuid>, PinNotReady> {
    if build_pin.is_some() {
        return Ok(build_pin);
    }
    match env {
        EnvironmentPin::Unset => Ok(None),
        EnvironmentPin::Ready(revision) => Ok(Some(*revision)),
        EnvironmentPin::NotReady(not_ready) => Err(not_ready.clone()),
    }
}

/// The environment `env_name` of app `app_id` pins, read in the environment's
/// semantic mode (see [`classify`]).
///
/// `Ready` only when the pinned revision exists, is `ready` and belongs to the
/// app's workspace — as for a build's pin, so a pin can never make a request
/// read another workspace's model. **A lookup error is `NotReady`**
/// ([`NotReadyCause::LookupFailed`], a retryable `503`): the request must not
/// quietly read another model because a read failed.
pub async fn environment_pin(
    db: &DatabaseConnection,
    app_id: Uuid,
    env_name: &str,
) -> EnvironmentPin {
    let sql = "SELECT r.revision_id AS ready_revision_id, e.semantic_git_sha AS git_sha, \
                      e.planes AS planes \
               FROM app_environments e \
               JOIN apps a ON a.id = e.app_id \
               LEFT JOIN revisions r ON r.revision_id = e.semantic_revision_id \
                    AND r.workspace_id = a.project_id AND r.status = 'ready' \
               WHERE e.app_id = $1 AND e.name = $2";
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [app_id.into(), env_name.into()],
        ))
        .await;
    match row
        .map_err(|e| e.to_string())
        .and_then(|row| row.map(read).transpose())
    {
        Ok(Some((semantic, ready, sha))) => classify(env_name, semantic, ready, sha),
        Ok(None) => EnvironmentPin::Unset,
        Err(error) => {
            tracing::warn!(%app_id, env_name, %error, "environment pin lookup failed; refusing the request");
            EnvironmentPin::NotReady(PinNotReady::lookup_failed(env_name))
        }
    }
}

/// What [`classify`] needs from one row: the semantic branch (or why the planes
/// do not parse), the ready pinned revision, the recorded commit.
type RowPin = (Result<Option<String>, String>, Option<Uuid>, Option<String>);

/// Read one row for [`classify`]; `Err` when a column cannot be decoded (the
/// same as a failed lookup).
fn read(r: sea_orm::QueryResult) -> Result<RowPin, String> {
    let planes = r
        .try_get::<serde_json::Value>("", "planes")
        .map_err(|e| e.to_string())?;
    let ready = r
        .try_get::<Option<Uuid>>("", "ready_revision_id")
        .map_err(|e| e.to_string())?;
    let sha = r
        .try_get::<Option<String>>("", "git_sha")
        .map_err(|e| e.to_string())?;
    Ok((semantic_branch(&planes), ready, sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: &str = "staging";

    fn not_ready(sha: Option<&str>) -> PinNotReady {
        PinNotReady::branch(ENV, "feat-x".into(), sha.map(str::to_string))
    }

    #[test]
    fn effective_pin_prefers_build() {
        let (build, env) = (Uuid::new_v4(), Uuid::new_v4());
        for pin in [
            EnvironmentPin::Ready(env),
            EnvironmentPin::NotReady(not_ready(Some("abc123"))),
            EnvironmentPin::Unset,
        ] {
            assert_eq!(effective_pin(Some(build), &pin), Ok(Some(build)), "{pin:?}");
        }
    }

    #[test]
    fn effective_pin_env_when_no_build() {
        let env = Uuid::new_v4();
        assert_eq!(
            effective_pin(None, &EnvironmentPin::Ready(env)),
            Ok(Some(env))
        );
    }

    #[test]
    fn effective_pin_not_ready_refuses() {
        let pin = EnvironmentPin::NotReady(not_ready(Some("abc123")));
        let refusal = effective_pin(None, &pin).unwrap_err();
        assert_eq!(refusal.git_sha(), "abc123");
        assert_eq!(refusal.environment, ENV);
        let fix = refusal.fix();
        assert!(fix.contains("abc123") && fix.contains("feat-x"), "{fix}");
        assert!(fix.contains("--plane semantic=build-pin"), "{fix}");
        assert_eq!(refusal.to_string(), fix);
    }

    #[test]
    fn effective_pin_unset_is_none() {
        assert_eq!(effective_pin(None, &EnvironmentPin::Unset), Ok(None));
    }

    fn branch() -> Result<Option<String>, String> {
        Ok(Some("feat-x".into()))
    }

    #[test]
    fn a_ready_revision_is_served_even_when_a_newer_commit_is_compiling() {
        let rev = Uuid::new_v4();
        assert_eq!(
            classify(ENV, branch(), Some(rev), Some("newer".into())),
            EnvironmentPin::Ready(rev)
        );
    }

    #[test]
    fn a_recorded_commit_with_no_ready_revision_is_not_ready() {
        for sha in ["abc"] {
            assert_eq!(
                classify(ENV, branch(), None, Some(sha.into())),
                EnvironmentPin::NotReady(not_ready(Some(sha)))
            );
        }
    }

    /// Branch mode with nothing pinned is refused, naming the branch: a failed
    /// pin after a planes write must not fall back to another model.
    #[test]
    fn branch_mode_with_no_pin_is_not_ready_and_names_the_branch() {
        for sha in [None, Some(String::new())] {
            let pin = classify(ENV, branch(), None, sha);
            assert_eq!(pin, EnvironmentPin::NotReady(not_ready(None)));
            let refusal = effective_pin(None, &pin).unwrap_err();
            assert_eq!(refusal.git_sha(), "");
            let fix = refusal.fix();
            assert!(fix.contains("semantic=branch:feat-x"), "{refusal}");
            assert!(fix.contains("no ready revision is pinned"), "{fix}");
        }
    }

    /// A mode that is not `branch` ignores the pin columns, whatever they hold.
    #[test]
    fn a_mode_that_is_not_branch_ignores_the_pin_columns() {
        let rev = Uuid::new_v4();
        for (revision, sha) in [
            (Some(rev), Some("abc".to_string())),
            (None, Some("abc".to_string())),
            (Some(rev), None),
            (None, None),
        ] {
            assert_eq!(
                classify(ENV, Ok(None), revision, sha),
                EnvironmentPin::Unset
            );
        }
    }

    /// Planes that do not parse are not ready, whatever the pin columns hold:
    /// the request is refused, never served the build's or production's model.
    #[test]
    fn planes_that_do_not_parse_are_not_ready() {
        let rev = Uuid::new_v4();
        let pin = classify(
            ENV,
            Err("unknown plane".into()),
            Some(rev),
            Some("a".into()),
        );
        assert_eq!(
            pin,
            EnvironmentPin::NotReady(PinNotReady::planes_unreadable(ENV))
        );
        let refusal = effective_pin(None, &pin).unwrap_err();
        assert_eq!(refusal.environment, ENV);
        assert_eq!(refusal.git_sha(), "");
        assert!(refusal.fix().contains("could not be read"), "{refusal}");
    }

    #[test]
    fn a_failed_lookup_is_a_retryable_refusal_naming_the_environment() {
        let refusal = PinNotReady::lookup_failed(ENV);
        assert_eq!(refusal.environment, ENV);
        assert!(refusal.fix().contains("retry shortly"), "{refusal}");
        let pin = EnvironmentPin::NotReady(refusal.clone());
        assert_eq!(effective_pin(None, &pin), Err(refusal));
    }

    #[test]
    fn the_branch_comes_from_the_semantic_choice_in_either_wire_form() {
        use serde_json::json;
        for (planes, expected) in [
            (json!({ "semantic": "branch:feat/x" }), Some("feat/x")),
            (
                json!({ "semantic": { "mode": "branch", "branch": "feat-x" } }),
                Some("feat-x"),
            ),
            (json!({ "semantic": "build-pin" }), None),
            (json!({}), None),
        ] {
            assert_eq!(
                semantic_branch(&planes).expect("parses").as_deref(),
                expected,
                "{planes}"
            );
        }
    }

    /// A value that does not parse is an error, not "names no branch".
    #[test]
    fn an_unparseable_value_is_an_error_not_no_branch() {
        use serde_json::json;
        for planes in [
            json!({ "semantic": "branch:a..b" }),
            json!({ "bogus": 1 }),
            json!({ "semantic": { "mode": "branch", "branch": "feat-x", "extra": 1 } }),
        ] {
            assert!(semantic_branch(&planes).is_err(), "{planes}");
        }
    }
}
