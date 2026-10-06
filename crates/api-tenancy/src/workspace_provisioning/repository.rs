//! The repository behind a workspace, as recorded on its row at creation.

use std::path::Path;

use oxy::github::GitHubRepository;
use uuid::Uuid;

/// The four repository columns of a `workspaces` row, in column order:
/// `git_namespace_id`, `git_remote_url`, `default_branch`, `repo_subdir`.
pub(super) type RepositoryColumns = (Option<Uuid>, Option<String>, Option<String>, Option<String>);

/// The git repository a workspace is imported from.
pub struct RepositoryOrigin {
    pub git_namespace_id: Uuid,
    pub remote_url: String,
    /// The repository's default branch — what `origin/HEAD` names once it is
    /// cloned. Not the branch the import checks out: `git clone --branch X`
    /// leaves `origin/HEAD` on the remote's default, and that is what the node
    /// holding the checkout answers with (`server::default_branch`). Recording
    /// X would have Postgres disagree with that node whenever they differ.
    pub default_branch: String,
    /// Where the workspace root sits inside the repository, `/`-separated.
    /// `None` at the repository root.
    pub subdir: Option<String>,
}

impl RepositoryOrigin {
    /// What the GitHub import knows before the clone starts. `subdir` is the
    /// validated relative path the caller chose (`onboarding::ops::parse_subdir`).
    pub fn from_github(
        git_namespace_id: Uuid,
        repo: &GitHubRepository,
        subdir: Option<&Path>,
    ) -> Self {
        Self {
            git_namespace_id,
            remote_url: repo.clone_url.clone(),
            default_branch: repo.default_branch.clone(),
            subdir: subdir.and_then(subdir_column),
        }
    }

    pub(super) fn into_columns(self) -> RepositoryColumns {
        (
            Some(self.git_namespace_id),
            Some(self.remote_url),
            Some(self.default_branch).filter(|b| !b.is_empty()),
            self.subdir.filter(|s| !s.is_empty()),
        )
    }
}

/// A relative path as `workspaces.repo_subdir` stores it: its components
/// joined by `/`, so `data/oxy/` and `data\oxy` both record `data/oxy` — the
/// form `oxy_git::cli::repo::subdir_in_repo` derives from a checkout.
fn subdir_column(subdir: &Path) -> Option<String> {
    let parts: Vec<_> = subdir
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    Some(parts.join("/")).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> GitHubRepository {
        GitHubRepository {
            id: 1,
            name: "analytics".into(),
            full_name: "acme/analytics".into(),
            default_branch: "trunk".into(),
            clone_url: "https://github.com/acme/analytics.git".into(),
        }
    }

    #[test]
    fn the_import_records_the_repository_default_branch_and_the_chosen_subdir() {
        let ns = Uuid::new_v4();
        let origin = RepositoryOrigin::from_github(ns, &repo(), Some(Path::new("data/oxy/")));
        assert_eq!(
            origin.into_columns(),
            (
                Some(ns),
                Some("https://github.com/acme/analytics.git".to_string()),
                Some("trunk".to_string()),
                Some("data/oxy".to_string()),
            )
        );
    }

    #[test]
    fn a_workspace_at_the_repository_root_records_no_subdir() {
        let origin = RepositoryOrigin::from_github(Uuid::new_v4(), &repo(), None);
        assert_eq!(origin.into_columns().3, None);
    }
}
