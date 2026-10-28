//! What a lazy self-heal compiles.
//!
//! A self-heal runs when a request finds no usable compiled config for a
//! workspace: it was never compiled, or the revision it serves was written
//! by a compiler whose output this build cannot read. It used to queue a
//! working-copy compile, which only the node holding the checkout can run and
//! which labels whatever is on that node's disk with a `local-<uuid>` identity.
//!
//! When the workspace serves a revision of a real commit that GitHub can
//! give back, it recompiles *that commit* instead, as a commit compile any
//! worker can take. The content is the same one the workspace was serving,
//! so a self-heal changes how it was compiled and nothing about what it says
//! (`internal-docs/factory-retirement.md`, phase 1).
//!
//! Anything else keeps the working-copy compile: a workspace with no
//! promoted revision (the first compile of a workspace is its own step), one
//! serving a `local-` revision, or one with no github.com remote or no
//! GitHub connection to fetch with.

use entity::{revisions, workspaces};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::{commit_sha, github_slug_of};
use crate::server::workspace_repo_facts::stored_default_branch;

/// The commit a self-heal recompiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub git_sha: String,
    pub branch: Option<String>,
}

/// What a self-heal does for a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heal {
    /// Recompile the commit the workspace serves.
    Commit(Target),
    /// The workspace has nothing compiled and a GitHub branch to compile: ask
    /// the periodic check to look at that branch now, rather than queue a
    /// compile only the Factory could run. Only while that check is switched
    /// on ([`crate::server::compile_reconcile::enabled`]).
    CheckBranch,
    /// Compile the working copy, as every self-heal used to.
    WorkingCopy,
}

/// What to do for a workspace. A database error is `WorkingCopy`: that is what
/// every self-heal did before this existed.
pub async fn heal(db: &DatabaseConnection, workspace_id: Uuid) -> Heal {
    let Ok(Some(row)) = workspaces::Entity::find_by_id(workspace_id).one(db).await else {
        return Heal::WorkingCopy;
    };
    let remote = row.git_remote_url.as_deref();
    let connected = row.git_namespace_id.is_some();
    let Some(promoted_id) = row.current_revision_id else {
        let may_check = crate::server::compile_reconcile::enabled();
        return if first_compile_by_check(remote, connected, &stored_default_branch(&row), may_check)
        {
            Heal::CheckBranch
        } else {
            Heal::WorkingCopy
        };
    };
    let Ok(Some(promoted)) = revisions::Entity::find_by_id(promoted_id).one(db).await else {
        return Heal::WorkingCopy;
    };
    if promoted.kind != "main" {
        return Heal::WorkingCopy;
    }
    // Only a revision a recompile can replace. The writer steps aside a ready
    // revision of the same commit only when its compiler is strictly older; for
    // an equal stamp (builds between version bumps) or a newer one (a rollback)
    // the recompile would lose to it again and again, and finish "cleanly" each
    // time, so no failure backoff would ever apply. Those keep the working-copy
    // compile, which always writes a fresh revision.
    if !oxy_compile::compiler_is_older(
        promoted.schema_version,
        &promoted.compiler_version,
        oxy_compile::CURRENT_SCHEMA_VERSION,
        &oxy_compile::compiler_version(),
    ) {
        return Heal::WorkingCopy;
    }
    plan(
        remote,
        connected,
        stored_default_branch(&row).or(promoted.branch),
        &promoted.git_sha,
    )
    .map_or(Heal::WorkingCopy, Heal::Commit)
}

/// Whether a workspace with nothing compiled gets its first compile from the
/// branch check. It needs everything the check itself needs: a github.com
/// remote, a GitHub connection and a recorded default branch.
pub(crate) fn first_compile_by_check(
    remote: Option<&str>,
    has_github_connection: bool,
    default_branch: &Option<String>,
    check_enabled: bool,
) -> bool {
    check_enabled
        && has_github_connection
        && default_branch.is_some()
        && remote.and_then(github_slug_of).is_some()
}

/// The decision, apart from the reads.
pub(crate) fn plan(
    remote: Option<&str>,
    has_github_connection: bool,
    branch: Option<String>,
    promoted_sha: &str,
) -> Option<Target> {
    github_slug_of(remote?)?;
    if !has_github_connection {
        return None;
    }
    // A `local-<uuid>` stamp, or anything that is not a full commit id, names
    // no commit GitHub can serve.
    let git_sha = commit_sha(Some(promoted_sha)).ok()?;
    Some(Target { git_sha, branch })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const REMOTE: &str = "https://github.com/acme/analytics.git";

    fn branch() -> Option<String> {
        Some("main".into())
    }

    #[test]
    fn a_workspace_serving_a_real_commit_recompiles_that_commit() {
        assert_eq!(
            plan(Some(REMOTE), true, branch(), SHA),
            Some(Target {
                git_sha: SHA.into(),
                branch: branch()
            })
        );
        // Upper case is normalised, as it is everywhere else.
        assert_eq!(
            plan(Some(REMOTE), true, None, &SHA.to_uppercase()).map(|t| t.git_sha),
            Some(SHA.to_string())
        );
    }

    #[test]
    fn a_local_revision_or_a_stamp_that_is_not_a_commit_keeps_the_working_copy_compile() {
        for stamp in [
            "local-6f1c2c1a-0000-4000-8000-000000000000",
            "",
            "main",
            "0123456",
        ] {
            assert_eq!(plan(Some(REMOTE), true, branch(), stamp), None, "{stamp:?}");
        }
    }

    #[test]
    fn a_workspace_with_nothing_compiled_waits_for_the_branch_check_only_when_it_can_run() {
        let main = Some("main".to_string());
        assert!(first_compile_by_check(Some(REMOTE), true, &main, true));
        // The check is off: the working-copy compile is the only first compile.
        assert!(!first_compile_by_check(Some(REMOTE), true, &main, false));
        // The check would skip it: nothing to ask GitHub about, or no way to.
        assert!(!first_compile_by_check(Some(REMOTE), true, &None, true));
        assert!(!first_compile_by_check(Some(REMOTE), false, &main, true));
        assert!(!first_compile_by_check(None, true, &main, true));
        assert!(!first_compile_by_check(
            Some("https://gitlab.com/acme/analytics.git"),
            true,
            &main,
            true
        ));
    }

    #[test]
    fn nothing_to_fetch_with_keeps_the_working_copy_compile() {
        assert_eq!(plan(None, true, branch(), SHA), None);
        assert_eq!(plan(Some(REMOTE), false, branch(), SHA), None);
        assert_eq!(
            plan(
                Some("https://gitlab.com/acme/analytics.git"),
                true,
                branch(),
                SHA
            ),
            None
        );
    }
}
