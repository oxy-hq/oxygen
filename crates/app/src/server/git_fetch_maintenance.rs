//! Keeps each workspace's `origin/*` remote-tracking refs warm.
//!
//! Everything that reports remote state — the Compile button's "Up to date"
//! badge, `revision-info`'s ahead/behind counts, the branch list — reads the
//! *locally cached* tracking ref (`git rev-parse origin/<branch>`), which is
//! only refreshed by an explicit fetch or pull. Nothing refreshed it on a
//! schedule, so those surfaces silently answered from whenever the user last
//! happened to fetch. That is how a workspace reported `behind: 0` while
//! demonstrably missing a commit that had been on origin for half an hour, and
//! why a freshness badge could not be trusted at all
//! (oxygen-workspace-sync-bugs.md bugs 1 and 3).
//!
//! This loop is deliberately **read-only with respect to the working copy**: it
//! runs `git fetch`, which updates `refs/remotes/*` and `FETCH_HEAD` and never
//! touches `HEAD`, the index, or any tracked file. It is therefore safe to run
//! underneath an editing user, mid-rebase, or on a dirty tree — unlike a pull,
//! which is why this is not one.
//!
//! Runs only where a working copy exists (`Ide` / `All`). A `Serve` replica has
//! no clone to fetch into, and a `Worker` has no reason to.

use std::time::Duration;

use oxy_git::GitClient;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::{debug, info, warn};

use crate::server::role_manifest::{Role, current_process_role};

const INTERVAL_ENV: &str = "OXY_GIT_FETCH_INTERVAL_SECS";
const DEFAULT_INTERVAL_SECS: u64 = 300;

/// Floor on the interval. Each tick is one `git fetch` per git-backed
/// workspace against the forge, so a small value turns into sustained
/// outbound traffic and, on GitHub App installs, rate-limit pressure.
const MIN_INTERVAL_SECS: u64 = 60;

/// Cap on how long a single workspace's fetch may take before it is abandoned
/// for this tick. An unreachable host would otherwise stall every workspace
/// behind it, so one bad remote cannot starve the rest.
const PER_WORKSPACE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct GitFetchMaintenanceConfig {
    pub interval: Duration,
}

impl GitFetchMaintenanceConfig {
    pub fn from_env() -> Self {
        let secs = std::env::var(INTERVAL_ENV)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_INTERVAL_SECS)
            .max(MIN_INTERVAL_SECS);
        Self {
            interval: Duration::from_secs(secs),
        }
    }
}

/// Spawn the detached fetch loop. No-op on roles without a working copy.
pub fn spawn_git_fetch_maintenance(config: GitFetchMaintenanceConfig) {
    let role = current_process_role();
    if !matches!(role, Role::Ide | Role::All) {
        debug!(role = role.as_str(), "git fetch maintenance: not this role");
        return;
    }

    tokio::spawn(async move {
        let db = match oxy::database::client::establish_connection().await {
            Ok(db) => db,
            Err(e) => {
                warn!(
                    ?e,
                    "git fetch maintenance: DB connect failed; loop not started"
                );
                return;
            }
        };
        info!(
            interval_secs = config.interval.as_secs(),
            "git fetch maintenance: started"
        );

        let mut tick = tokio::time::interval(config.interval);
        // A sweep is sequential with a per-workspace timeout, so on a node
        // holding many workspaces with slow or unreachable remotes it can
        // outrun the interval. Under the default `Burst` behaviour the missed
        // ticks then fire back-to-back and the freshness sweep degenerates into
        // continuous fetching — exactly the load the timeout exists to bound.
        // `Delay` keeps the intended spacing from the end of each sweep.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; skip it so startup isn't competing
        // with clone/migration work for the same remotes.
        tick.tick().await;
        loop {
            tick.tick().await;
            fetch_all_workspaces(&db).await;
        }
    });
}

async fn fetch_all_workspaces(db: &DatabaseConnection) {
    let workspaces = match entity::workspaces::Entity::find()
        .filter(entity::workspaces::Column::GitRemoteUrl.is_not_null())
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(?e, "git fetch maintenance: workspace query failed");
            return;
        }
    };

    let (mut ok, mut failed, mut unlinked) = (0u32, 0u32, 0u32);
    for ws in workspaces {
        // Sequential on purpose: these are network calls against (usually) the
        // same forge, and this is a background freshness sweep with no deadline.
        // Fanning out would add rate-limit pressure to buy latency nobody waits on.
        match fetch_one(db, &ws).await {
            Ok(Outcome::Fetched) => ok += 1,
            Ok(Outcome::Unlinked) => unlinked += 1,
            Ok(Outcome::Nothing) => {}
            Err(e) => {
                failed += 1;
                // Debug, not warn: a workspace whose remote is unreachable or
                // whose token has lapsed would otherwise log on every tick
                // forever. The staleness is already visible in the UI, which is
                // the signal that matters.
                debug!(workspace_id = %ws.id, error = %e, "git fetch maintenance: fetch failed");
            }
        }
    }
    if ok > 0 || failed > 0 || unlinked > 0 {
        // `unlinked` is counted separately rather than folded into either
        // bucket: it is neither a healthy fetch nor a broken remote, and it is
        // the one outcome an operator can act on — those workspaces need a
        // GitHub connection linked before their freshness badge means anything.
        debug!(
            ok,
            failed, unlinked, "git fetch maintenance: sweep complete"
        );
    }
}

/// What a single workspace's sweep did.
enum Outcome {
    /// `origin/<default branch>` was refreshed.
    Fetched,
    /// Nothing to do: no working copy on this node, not a repo, no remote.
    Nothing,
    /// A git remote is configured but no GitHub connection is linked, so there
    /// is no token to fetch with. Distinct from `Nothing` because it is a
    /// misconfiguration a human can fix, not a structural no-op.
    Unlinked,
}

/// Fetch one workspace's default branch.
async fn fetch_one(
    db: &DatabaseConnection,
    ws: &entity::workspaces::Model,
) -> Result<Outcome, oxy_shared::errors::OxyError> {
    let Some(path) = ws.path.as_deref() else {
        return Ok(Outcome::Nothing);
    };
    let path = std::path::Path::new(path);
    // A replica may hold a row for a workspace it has never cloned.
    if !path.exists() {
        return Ok(Outcome::Nothing);
    }

    let git = oxy::github::default_git_client();
    if !git.is_git_repo(path) || !git.has_remote(path).await {
        return Ok(Outcome::Nothing);
    }

    let branch = git.get_default_branch(path).await;
    if branch.is_empty() {
        return Ok(Outcome::Nothing);
    }

    // Every remote-backed workspace with a checkout passes through here once
    // an interval, opened or not — which makes this the one place that reaches
    // a workspace nobody touches. Its default branch and subdirectory are
    // otherwise recorded only when something asks for its default branch, and
    // the compile reconcile cannot check a workspace whose branch is unknown.
    crate::server::workspace_repo_facts::record_from_checkout(db, ws, path, &branch).await;

    // No requesting user here, so only the workspace's own `git_namespace_id`
    // link can supply a token. Skip rather than fetch unauthenticated: git no
    // longer falls back to the host's credential helper, so an unlinked
    // workspace would fail every tick — a private repo can only 401, and a
    // public one does not need the round trip.
    let Some(token) = oxy::github::github_token_for_workspace(ws).await? else {
        debug!(
            workspace_id = %ws.id,
            "git fetch maintenance: no linked git namespace; skipping"
        );
        return Ok(Outcome::Unlinked);
    };
    let fetch = git.fetch_remote_ref(path, &branch, Some(&token));
    match tokio::time::timeout(PER_WORKSPACE_TIMEOUT, fetch).await {
        Ok(result) => result.map(|()| Outcome::Fetched),
        Err(_) => Err(oxy_shared::errors::OxyError::RuntimeError(format!(
            "fetch timed out after {}s",
            PER_WORKSPACE_TIMEOUT.as_secs()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use uuid::Uuid;

    use super::{Outcome, fetch_one};
    use crate::server::test_support::{SKIP_MSG, test_db};

    fn git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The workspace the reconcile loop exists for is one nobody opens, so
    /// nothing ever asks for its default branch. The sweep reaches it anyway.
    /// No GitHub connection is linked, so the sweep stops before any network.
    #[tokio::test]
    async fn the_sweep_records_the_repository_facts_of_a_workspace_nobody_opened() {
        let Some(db) = test_db().await else {
            eprintln!("{SKIP_MSG}");
            return;
        };
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::remove_var("GIT_DEFAULT_BRANCH") };
        let repo = tempfile::tempdir().expect("tempdir");
        git(repo.path(), &["init", "-q", "-b", "trunk"]);
        git(
            repo.path(),
            &["commit", "-q", "--allow-empty", "-m", "init"],
        );
        let origin = "https://example.invalid/acme/analytics.git";
        git(repo.path(), &["remote", "add", "origin", origin]);
        git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/trunk", "HEAD"],
        );
        let head = [
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ];
        git(repo.path(), &head);
        let workspace = repo.path().join("data").join("oxy");
        std::fs::create_dir_all(&workspace).expect("workspace dir");

        let id = Uuid::new_v4();
        let row = entity::workspaces::ActiveModel {
            id: Set(id),
            name: Set(format!("fetch-sweep-{id}")),
            path: Set(Some(workspace.to_string_lossy().into_owned())),
            git_remote_url: Set(Some(origin.into())),
            status: Set(entity::workspaces::WorkspaceStatus::Ready),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("seed workspace");

        let outcome = fetch_one(&db, &row).await.expect("sweep one workspace");

        assert!(matches!(outcome, Outcome::Unlinked));
        let recorded = entity::workspaces::Entity::find_by_id(id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recorded.default_branch.as_deref(), Some("trunk"));
        assert_eq!(recorded.repo_subdir.as_deref(), Some("data/oxy"));
    }
}
