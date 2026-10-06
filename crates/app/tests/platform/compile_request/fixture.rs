//! The process, the fake GitHub and the workspace the `compile_request` tests
//! run against.

use std::path::Path;

use chrono::Utc;
use entity::{revisions, workspaces};
use oxy::workspace_fs_probe::{process_owns_workspace_files, reset_leaks};
use oxy_app::server::compile_request::{self, CompileState, Refusal, Target};
use oxy_app::server::role_manifest::init_process_role_from_env;
use oxy_compile::RevisionKind;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use serde_json::Value;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{Schema, test_db_with};
use crate::compile_from_git::{TOKEN, drive_worker_queue, seed_workspace};

/// Where `feat/x` is on GitHub.
pub const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
/// What the workspace serves: a main revision of another commit.
pub const SERVED: &str = "1111111111111111111111111111111111111111";
pub const BRANCH: &str = "feat/x";

pub struct Fx {
    pub db: DatabaseConnection,
    pub ws: Uuid,
    pub github: MockServer,
    /// The main revision the workspace serves, promoted before the test.
    pub served: Uuid,
}

/// One compile task of the workspace, as the queue holds it.
#[derive(Debug)]
pub struct Queued {
    pub spec: Value,
    pub queue_status: String,
    pub source_type: String,
}

impl Fx {
    /// A process of `role` (`None` is the default, `all`, which owns working
    /// copies), a fake GitHub, and a remote-backed workspace serving a main
    /// revision.
    pub async fn new(role: Option<&str>) -> Self {
        // `test_db_with` also points the process-wide connection at this
        // database: the token lookup goes through it, not through `db`.
        let db = test_db_with(Schema::All).await;
        let github = MockServer::start().await;
        // SAFETY: nextest runs each test in its own process (`test_db_with`
        // asserts it), and nothing has read these yet.
        unsafe {
            match role {
                Some(role) => std::env::set_var("OXY_ROLE", role),
                None => std::env::remove_var("OXY_ROLE"),
            }
            std::env::set_var("GITHUB_API_URL", github.uri());
            std::env::remove_var("OXY_COMPILE_BLOB_S3_BUCKET");
        }
        init_process_role_from_env();
        assert_eq!(process_owns_workspace_files(), role.is_none());

        let ws = seed_workspace(&db, None).await;
        let mut fx = Self {
            db,
            ws,
            github,
            served: Uuid::nil(),
        };
        fx.served = fx.ready_revision(SERVED, "main").await;
        fx.edit_workspace(|w| w.current_revision_id = ActiveValue::Set(Some(fx.served)))
            .await;
        reset_leaks();
        fx
    }

    pub async fn workspace(&self) -> workspaces::Model {
        workspaces::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .expect("read workspace")
            .expect("workspace exists")
    }

    pub async fn edit_workspace(&self, edit: impl FnOnce(&mut workspaces::ActiveModel)) {
        let mut row: workspaces::ActiveModel = self.workspace().await.into();
        edit(&mut row);
        row.update(&self.db).await.expect("update workspace");
    }

    /// GitHub's answer to "where is `branch`", for a caller carrying the token.
    pub async fn github_answers(&self, branch: &str, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(format!(
                "/repos/acme/analytics/commits/heads/{branch}"
            )))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(response)
            .mount(&self.github)
            .await;
    }

    pub async fn branch_is_at(&self, branch: &str, sha: &str) {
        self.github_answers(branch, ResponseTemplate::new(200).set_body_string(sha))
            .await;
    }

    /// Serve `body` as the archive of `sha`.
    pub async fn serve_commit(&self, sha: &str, body: Vec<u8>) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/acme/analytics/tarball/{sha}")))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&self.github)
            .await;
    }

    pub async fn compile(&self, target: Target<'_>) -> Result<CompileState, Refusal> {
        let workspace = self.workspace().await;
        compile_request::compile(&self.db, &workspace, target, RevisionKind::Staging).await
    }

    pub async fn status(&self, sha: &str) -> CompileState {
        compile_request::status(&self.db, self.ws, sha, RevisionKind::Staging)
            .await
            .expect("read status")
    }

    /// Every compile task of the workspace, oldest first.
    pub async fn queued(&self) -> Vec<Queued> {
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT q.spec, q.queue_status, r.source_type FROM agentic_task_queue q \
                 JOIN agentic_runs r ON r.id = q.run_id \
                 WHERE q.spec->>'type' = 'compile' AND q.spec->>'workspace_id' = $1 \
                 ORDER BY q.created_at",
                [self.ws.to_string().into()],
            ))
            .await
            .expect("read the queue");
        rows.iter()
            .map(|r| Queued {
                spec: r.try_get("", "spec").unwrap(),
                queue_status: r.try_get("", "queue_status").unwrap(),
                source_type: r.try_get("", "source_type").unwrap(),
            })
            .collect()
    }

    pub async fn promoted(&self) -> Option<Uuid> {
        self.workspace().await.current_revision_id
    }

    /// A ready revision of `git_sha` this build of the compiler would reuse.
    pub async fn ready_revision(&self, git_sha: &str, kind: &str) -> Uuid {
        let now = Utc::now().fixed_offset();
        let id = Uuid::new_v4();
        revisions::ActiveModel {
            revision_id: ActiveValue::Set(id),
            workspace_id: ActiveValue::Set(self.ws),
            git_sha: ActiveValue::Set(git_sha.into()),
            branch: ActiveValue::Set(Some("main".into())),
            schema_version: ActiveValue::Set(oxy_compile::CURRENT_SCHEMA_VERSION),
            status: ActiveValue::Set("ready".into()),
            kind: ActiveValue::Set(kind.into()),
            owner_user_id: ActiveValue::Set(None),
            compiler_version: ActiveValue::Set(oxy_compile::compiler_version()),
            started_at: ActiveValue::Set(now),
            finished_at: ActiveValue::Set(Some(now)),
            file_count_seen: ActiveValue::Set(1),
            file_count_compiled: ActiveValue::Set(1),
            file_count_failed: ActiveValue::Set(0),
            error_summary: ActiveValue::Set(None),
        }
        .insert(&self.db)
        .await
        .expect("seed revision");
        id
    }

    pub async fn revisions_of(&self, git_sha: &str) -> Vec<revisions::Model> {
        revisions::Entity::find()
            .filter(revisions::Column::WorkspaceId.eq(self.ws))
            .filter(revisions::Column::GitSha.eq(git_sha))
            .all(&self.db)
            .await
            .expect("read revisions")
    }

    /// Let a worker drain the queue until every compile task has ended.
    pub async fn drive_until_settled(&self) {
        let settled = drive_worker_queue(&self.db, self.ws, || async {
            let queued = self.queued().await;
            !queued.is_empty()
                && queued
                    .iter()
                    .all(|q| !matches!(q.queue_status.as_str(), "queued" | "claimed"))
        })
        .await;
        assert!(
            settled,
            "the queue did not settle: {:?}",
            self.queued().await
        );
    }

    pub async fn github_requests(&self) -> usize {
        self.github
            .received_requests()
            .await
            .unwrap_or_default()
            .len()
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository on `main` with `feat/x` and `local-only` one commit ahead of
/// it each. Returns the directory and the two branch heads.
pub fn working_copy() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("config.yml"), "models: []\ndatabases: []\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "config"]);
    let mut heads = Vec::new();
    for branch in [BRANCH, "local-only"] {
        git(root, &["checkout", "-qb", branch, "main"]);
        git(
            root,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                &format!("on {branch}"),
            ],
        );
        heads.push(git(root, &["rev-parse", "HEAD"]));
    }
    git(root, &["checkout", "-q", "main"]);
    let local_only = heads.pop().unwrap();
    (dir, heads.pop().unwrap(), local_only)
}
