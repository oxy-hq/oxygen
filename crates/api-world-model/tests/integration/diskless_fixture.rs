//! What `world_model_diskless` needs to stand up a compiled workspace and then
//! take its working copy away: an org, a per-test Postgres warehouse, the
//! compile, and the process posing as a `serve` replica.
//!
//! The test came from `oxy-api-custom-apps`' integration group with the graph
//! handlers, and these are the pieces of that group's fixtures it used, cut to
//! what a graph read needs. A surface crate may not depend on another, not even
//! for a test (`surface_contract.rs`), so they are copied rather than shared.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use entity::{organizations, workspaces};
use oxy::workspace_fs_probe::{reset_leaks, set_process_owns_workspace_files};
use oxy_app::surface::testing::compile_config_gate::runtime_config_gate;
use oxy_compile::{CompileRequest, Promotion, RevisionKind, compile_workspace, compiler_version};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

use crate::common::{Schema, fresh_db, test_db_with};

/// A per-test database and one org in it.
pub(crate) struct Tenant {
    pub(crate) db: DatabaseConnection,
    pub(crate) org_id: Uuid,
}

/// A fresh database with every migrator applied (the lazy compile a refusal
/// queues lands in `agentic_task_queue`) and a fresh org in it.
pub(crate) async fn tenant() -> Tenant {
    let db = test_db_with(Schema::All).await;
    let org_id = Uuid::new_v4();
    let slug = format!("wm-diskless-{}", &org_id.simple().to_string()[..8]);
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(slug.clone()),
        slug: ActiveValue::Set(slug),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed an org");
    Tenant { db, org_id }
}

/// A registered workspace row in the tenant's org, never compiled, whose
/// directory is not on this node.
pub(crate) async fn uncompiled_workspace(t: &Tenant) -> Uuid {
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace),
        name: ActiveValue::Set("Never compiled".into()),
        org_id: ActiveValue::Set(Some(t.org_id)),
        path: ActiveValue::Set(Some(format!("/nonexistent/workspaces/{workspace}"))),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");
    workspace
}

/// Compile tasks queued for `workspace`'s main revision.
pub(crate) async fn queued_compiles(t: &Tenant, workspace: Uuid) -> i64 {
    let row =
        t.db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM agentic_task_queue \
             WHERE spec->>'type' = 'compile' AND spec->>'workspace_id' = $1",
            [workspace.to_string().into()],
        ))
        .await
        .expect("query the task queue")
        .expect("count returns a row");
    row.try_get::<i64>("", "n").expect("n")
}

/// The process as a `serve` replica: it owns no working copy, and the leak
/// counter starts from zero. Restored on drop; nextest gives each test its own
/// process (`common::fresh_db` asserts it), so the flag reaches no other test.
pub(crate) struct AsDisklessReplica;

impl AsDisklessReplica {
    pub(crate) fn enter() -> Self {
        set_process_owns_workspace_files(false);
        reset_leaks();
        AsDisklessReplica
    }
}

impl Drop for AsDisklessReplica {
    fn drop(&mut self) {
        set_process_owns_workspace_files(true);
        reset_leaks();
    }
}

/// A workspace directory holding only a `config.yml` with `databases`.
pub(crate) fn write_config(databases: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("workspace dir");
    std::fs::write(
        root.path().join("config.yml"),
        format!("databases:\n{databases}models: []\n"),
    )
    .expect("write config.yml");
    root
}

/// A workspace row in the tenant's org at `root`, compiled and promoted.
pub(crate) async fn compile(t: &Tenant, root: &Path) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("World model".into()),
        org_id: ActiveValue::Set(Some(t.org_id)),
        path: ActiveValue::Set(Some(root.to_string_lossy().into_owned())),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");
    let outcome = compile_workspace(CompileRequest {
        db: &t.db,
        workspace_id: id,
        workspace_path: root,
        git_sha: None,
        branch: None,
        compiler_version: compiler_version(),
        promote: true,
        kind: RevisionKind::Main,
        owner_user_id: None,
        config_gate: Some(runtime_config_gate()),
    })
    .await
    .expect("compile the workspace");
    assert_eq!(
        outcome.promotion,
        Promotion::Promoted,
        "the runtime reads the promoted revision; failures: {:?}",
        outcome.failures
    );
    id
}

/// A per-test Postgres database, as a `config.yml` entry named `name`.
///
/// **The password goes in by reference, never inline.** `oxy-compile` redacts
/// an inline `password:` literal before the compiled config reaches Postgres,
/// so a workspace the runtime reads through the compile boundary would
/// authenticate with an empty password (`28P01`). A `password_var` names a
/// secret instead, and `SecretsManager` falls back to the environment when the
/// project's store has no such row.
pub(crate) async fn postgres_entry(name: &str) -> String {
    let (_db, url) = fresh_db(Schema::Central).await;
    let url = url::Url::parse(&url).expect("fresh_db hands back a URL");
    let var = park_password(name, url.password().unwrap_or_default());
    format!(
        "  - name: {name}\n    type: postgres\n    host: {host}\n    port: \"{port}\"\n    \
         user: {user}\n    password_var: {var}\n    database: {database}\n",
        host = url.host_str().expect("host"),
        port = url.port().unwrap_or(5432),
        user = url.username(),
        database = url.path().trim_start_matches('/'),
    )
}

/// Parks `password` in an env var unique to this call and returns its name. One
/// fixed name would let a second entry overwrite the first one's password.
fn park_password(name: &str, password: &str) -> String {
    static NTH: AtomicUsize = AtomicUsize::new(0);
    let slug = name
        .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        .to_ascii_uppercase();
    let var = format!(
        "OXY_TEST_WAREHOUSE_PG_PASSWORD_{slug}_{}",
        NTH.fetch_add(1, Ordering::Relaxed)
    );
    // SAFETY: nextest runs each test in its own process, and this is set before
    // any connector in it resolves the variable.
    unsafe { std::env::set_var(&var, password) };
    var
}
