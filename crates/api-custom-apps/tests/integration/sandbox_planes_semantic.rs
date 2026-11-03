//! The `semantic` plane through the environment planes routes
//! (`PUT …/planes`, `POST …/environments`) and the pin port they call
//! (`oxy_app_core::semantic_pin_seam`): a `branch:<name>` choice asks the
//! workspace for the branch before it is stored, pins the environment after,
//! and any other choice (or a different branch) drops the pin the environment
//! had, in the same transaction as the store.
//!
//! The port is faked here (the real one, over a wiremock GitHub, is
//! `oxy-app`'s `platform` test `env_pins`); the fake writes the same columns
//! the real `pin_to_branch` does (**both** pin columns, `semantic_revision_id`
//! and `semantic_git_sha`: the reader serves the revision, so a test that
//! looked at the sha alone would pass a store that kept the old revision). A
//! surface crate may reach `oxy-app` only through its prelude, so it cannot
//! call it; what is checked in the database is what the reader sees.

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use oxy_app_core::semantic_pin_seam::{PinRefusal, SemanticPinner};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_routes::{audit_rows, install_pinner, send, staffed_app};

/// The one branch the fake workspace has.
const BRANCH: &str = "feat-x";

/// A pin port over a workspace whose only branch is [`BRANCH`]; records what it
/// was asked, and fails `pin` while `pin_down` is set.
struct FakePinner {
    db: DatabaseConnection,
    extra_branches: Mutex<Vec<String>>,
    calls: Mutex<Vec<String>>,
    pin_down: Mutex<bool>,
}

impl FakePinner {
    fn install(db: &DatabaseConnection, pin_down: bool) -> Arc<FakePinner> {
        let pinner = Arc::new(FakePinner {
            db: db.clone(),
            extra_branches: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            pin_down: Mutex::new(pin_down),
        });
        install_pinner(pinner.clone());
        pinner
    }

    /// A second branch the fake workspace has.
    fn also_has(&self, branch: &str) {
        self.extra_branches
            .lock()
            .expect("branches")
            .push(branch.to_string());
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().expect("calls").push(call);
    }
}

#[async_trait::async_trait]
impl SemanticPinner for FakePinner {
    async fn check_branch(&self, workspace_id: Uuid, branch: &str) -> Result<(), PinRefusal> {
        assert_eq!(workspace_id, demo_workspace_id(), "the app's workspace");
        self.record(format!("check {branch}"));
        if branch == BRANCH
            || self
                .extra_branches
                .lock()
                .expect("branches")
                .contains(&branch.to_string())
        {
            return Ok(());
        }
        Err(PinRefusal::new(
            StatusCode::NOT_FOUND,
            "unknown_branch",
            format!("branch {branch:?} does not exist in workspace {workspace_id}"),
        ))
    }

    async fn pin(
        &self,
        workspace_id: Uuid,
        app_id: Uuid,
        env_name: &str,
        branch: &str,
    ) -> Result<(), PinRefusal> {
        assert_eq!(workspace_id, demo_workspace_id(), "the app's workspace");
        self.record(format!("pin {env_name} {branch}"));
        if *self.pin_down.lock().expect("pin_down") {
            return Err(PinRefusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "github_unavailable",
                "could not ask GitHub where the branch is",
            ));
        }
        // What `pin_to_branch` leaves for a head that is already compiled: a
        // ready revision and the commit it was compiled from.
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE app_environments \
                 SET semantic_git_sha = $3, semantic_revision_id = $4 \
                 WHERE app_id = $1 AND name = $2",
                [
                    app_id.into(),
                    env_name.into(),
                    "a".repeat(40).into(),
                    Uuid::new_v4().into(),
                ],
            ))
            .await
            .expect("write the pin");
        Ok(())
    }
}

async fn put_planes(app: Uuid, name: &str, planes: Value) -> (StatusCode, Value) {
    send(
        "PUT",
        &format!("{app}/environments/{name}/planes"),
        Some(json!({ "planes": planes })),
    )
    .await
}

async fn create(app: Uuid, body: Value) -> (StatusCode, Value) {
    send("POST", &format!("{app}/environments"), Some(body)).await
}

/// An environment's `planes` and both pin columns, as the reader sees them.
#[derive(Debug, PartialEq)]
struct PinRow {
    planes: Value,
    revision: Option<Uuid>,
    sha: Option<String>,
}

impl PinRow {
    /// Both columns set: what a pin leaves.
    fn is_pinned(&self) -> bool {
        self.revision.is_some() && self.sha.is_some()
    }
}

/// A row storing `planes` with **both** pin columns clear.
fn unpinned(planes: Value) -> PinRow {
    PinRow {
        planes,
        revision: None,
        sha: None,
    }
}

async fn row(db: &DatabaseConnection, app: Uuid, name: &str) -> Option<PinRow> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT planes, semantic_revision_id, semantic_git_sha FROM app_environments \
         WHERE app_id = $1 AND name = $2",
        [app.into(), name.into()],
    ))
    .await
    .expect("read the row")
    .map(|r| PinRow {
        planes: r.try_get("", "planes").expect("planes"),
        revision: r.try_get("", "semantic_revision_id").expect("revision"),
        sha: r.try_get("", "semantic_git_sha").expect("sha"),
    })
}

/// A branch the workspace does not have is `400 invalid_planes` naming it,
/// before anything is stored, pinned or audited.
#[tokio::test]
async fn an_unknown_branch_is_a_400_that_names_it_and_stores_nothing() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let pinner = FakePinner::install(&t.db, false);
    create(app, json!({ "name": "dev-a1" })).await;
    let kept = json!({ "workspace-actions": "held" });
    put_planes(app, "dev-a1", kept.clone()).await;

    let (status, refused) = put_planes(app, "dev-a1", json!({ "semantic": "branch:nope" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "invalid_planes");
    assert!(
        refused["message"].as_str().unwrap_or("").contains("nope"),
        "names the branch: {refused}"
    );
    assert_eq!(row(&t.db, app, "dev-a1").await, Some(unpinned(kept)));
    assert!(
        !pinner.calls().iter().any(|c| c.starts_with("pin ")),
        "{:?}",
        pinner.calls()
    );

    // A create refuses the same way, and leaves no sandbox.
    let (status, refused) = create(
        app,
        json!({ "name": "dev-b2", "planes": { "semantic": "branch:nope" } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"], "invalid_planes");
    assert_eq!(row(&t.db, app, "dev-b2").await, None);
}

/// A known branch is stored as written, shown in `planes_set` as
/// `branch:<b>`, and the environment is pinned; a later choice that is not a
/// branch clears the pin without calling the port: the store drops it.
#[tokio::test]
async fn a_known_branch_is_stored_and_pinned_and_leaving_it_clears_the_pin() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let pinner = FakePinner::install(&t.db, false);
    create(app, json!({ "name": "dev-a1" })).await;

    // No pin yet, no branch: nothing to clear, so the port is not asked.
    let (status, plain) = put_planes(app, "dev-a1", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    assert_eq!(pinner.calls(), Vec::<String>::new());

    let set = json!({ "semantic": "branch:feat-x" });
    let (status, shown) = put_planes(app, "dev-a1", set.clone()).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["planes_set"]["semantic"], "branch:feat-x");
    assert_eq!(shown["planes"]["semantic"], "branch");
    let stored = row(&t.db, app, "dev-a1").await.expect("row");
    assert_eq!(stored.planes, set);
    assert!(stored.is_pinned(), "both columns set: {stored:?}");
    assert_eq!(pinner.calls(), ["check feat-x", "pin dev-a1 feat-x"]);
    assert_eq!(
        audit_rows(&t.db, app, "app.environment.planes_set").await,
        2
    );

    let (status, cleared) = put_planes(app, "dev-a1", json!({ "semantic": "build-pin" })).await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["planes"]["semantic"], "build-pin");
    assert_eq!(
        row(&t.db, app, "dev-a1").await,
        Some(unpinned(json!({ "semantic": "build-pin" }))),
        "leaving the branch cleared the revision and the sha"
    );
    assert_eq!(
        pinner.calls(),
        ["check feat-x", "pin dev-a1 feat-x"],
        "the store dropped the pin: no clear call"
    );
}

/// The object form is stored as sent and pins the same branch.
#[tokio::test]
async fn the_object_form_pins_the_same_branch() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let pinner = FakePinner::install(&t.db, false);
    create(app, json!({ "name": "dev-a1" })).await;
    let set = json!({ "semantic": { "mode": "branch", "branch": "feat-x" } });
    let (status, shown) = put_planes(app, "dev-a1", set.clone()).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["planes_set"], set, "the stored object, as written");
    assert_eq!(shown["planes"]["semantic"], "branch");
    assert_eq!(pinner.calls(), ["check feat-x", "pin dev-a1 feat-x"]);
}

/// A pin that fails after the store is answered with its own status, leaves
/// the planes stored and no pin, and the same request sent again pins.
#[tokio::test]
async fn a_failed_pin_is_answered_after_the_store_and_a_retry_pins() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let pinner = FakePinner::install(&t.db, true);
    create(app, json!({ "name": "dev-a1" })).await;
    let set = json!({ "semantic": "branch:feat-x" });

    let (status, failed) = put_planes(app, "dev-a1", set.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{failed}");
    assert_eq!(failed["error"], "github_unavailable");
    assert!(
        failed["message"].as_str().unwrap_or("").contains("again"),
        "says how to retry: {failed}"
    );
    assert_eq!(row(&t.db, app, "dev-a1").await, Some(unpinned(set.clone())));

    *pinner.pin_down.lock().expect("pin_down") = false;
    let (status, shown) = put_planes(app, "dev-a1", set).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert!(row(&t.db, app, "dev-a1").await.expect("row").is_pinned());
}

/// A create with a known branch pins the new sandbox, and a create naming no
/// branch asks the pinner nothing. `preset: full` asks for an OLTP branch, so
/// on an org with no OLTP database it is refused (`400 invalid_planes`)
/// before anything is stored; `workspace-actions: preview`, which `full`
/// carries, is stored when asked for by name.
#[tokio::test]
async fn a_create_pins_its_branch_and_a_preview_create_asks_the_pinner_nothing() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let pinner = FakePinner::install(&t.db, false);

    let (status, created) = create(
        app,
        json!({ "name": "dev-a1", "planes": { "semantic": "branch:feat-x" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["planes_set"]["semantic"], "branch:feat-x");
    assert!(row(&t.db, app, "dev-a1").await.expect("row").is_pinned());
    assert_eq!(pinner.calls(), ["check feat-x", "pin dev-a1 feat-x"]);

    let (status, full) = create(app, json!({ "name": "dev-b2", "preset": "full" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{full}");
    assert_eq!(full["error"], "invalid_planes", "{full}");
    assert!(
        full["message"].as_str().unwrap_or("").contains("OLTP"),
        "{full}"
    );
    assert_eq!(row(&t.db, app, "dev-b2").await, None, "nothing stored");

    let (status, preview) = create(
        app,
        json!({ "name": "dev-b2", "planes": { "workspace-actions": "preview" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    assert_eq!(
        preview["planes_set"],
        json!({ "workspace-actions": "preview" })
    );
    assert_eq!(preview["planes"]["workspace-actions"], "preview");
    assert_eq!(preview["planes"]["semantic"], "build-pin");
    assert_eq!(
        pinner.calls().len(),
        2,
        "a create naming no branch asks nothing"
    );
}

/// With no pin port installed, a branch choice is `503
/// semantic_pin_unavailable` and nothing is stored; every write that needs no
/// pin still works.
#[tokio::test]
async fn without_a_pin_port_a_branch_is_a_503_and_other_writes_still_work() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    create(app, json!({ "name": "dev-a1" })).await;

    let (status, refused) = put_planes(app, "dev-a1", json!({ "semantic": "branch:feat-x" })).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
    assert_eq!(refused["error"], "semantic_pin_unavailable");
    assert_eq!(row(&t.db, app, "dev-a1").await, Some(unpinned(json!({}))));

    let (status, refused) = create(
        app,
        json!({ "name": "dev-b2", "planes": { "semantic": "branch:feat-x" } }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
    assert_eq!(row(&t.db, app, "dev-b2").await, None);

    let (status, ok) = put_planes(app, "dev-a1", json!({ "semantic": "build-pin" })).await;
    assert_eq!(status, StatusCode::OK, "{ok}");

    // Leaving a branch needs no port: the store drops the pin itself, so a
    // missing port cannot leave an old branch's pin under the new choice.
    t.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_environments SET semantic_git_sha = $3, semantic_revision_id = $4 \
         WHERE app_id = $1 AND name = $2",
        [
            app.into(),
            "dev-a1".into(),
            "b".repeat(40).into(),
            Uuid::new_v4().into(),
        ],
    ))
    .await
    .expect("seed a pin");
    let (status, ok) = put_planes(app, "dev-a1", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{ok}");
    assert_eq!(row(&t.db, app, "dev-a1").await, Some(unpinned(json!({}))));
}

/// Repointing `branch:a` to `branch:b` clears the pin A left, in the same
/// transaction as the store, before `pin` runs: a pin that then fails leaves
/// the row on branch B with no pin (refused by the reader), never branch A's
/// revision under a row that says B. A retry of the same request pins.
#[tokio::test]
async fn a_repoint_never_leaves_the_old_branchs_pin_behind() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let pinner = FakePinner::install(&t.db, false);
    pinner.also_has("feat-y");
    create(app, json!({ "name": "dev-a1" })).await;
    let (status, shown) = put_planes(app, "dev-a1", json!({ "semantic": "branch:feat-x" })).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert!(row(&t.db, app, "dev-a1").await.expect("row").is_pinned());

    *pinner.pin_down.lock().expect("pin_down") = true;
    let to_y = json!({ "semantic": "branch:feat-y" });
    let (status, failed) = put_planes(app, "dev-a1", to_y.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{failed}");
    assert_eq!(
        row(&t.db, app, "dev-a1").await,
        Some(unpinned(to_y.clone())),
        "branch feat-y is stored and feat-x's revision and sha are both gone"
    );

    // The same branch again does not clear: it pins.
    *pinner.pin_down.lock().expect("pin_down") = false;
    let (status, shown) = put_planes(app, "dev-a1", to_y).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert!(row(&t.db, app, "dev-a1").await.expect("row").is_pinned());
}

/// A create whose pin fails still made the sandbox, so its answer names the
/// retry that works — a planes write — and that write pins it; the create
/// itself sent again is `409 environment_exists`.
#[tokio::test]
async fn a_failed_pin_on_create_names_the_planes_write_that_pins_it() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    let pinner = FakePinner::install(&t.db, true);
    let set = json!({ "semantic": "branch:feat-x" });
    let body = json!({ "name": "dev-a1", "planes": set.clone() });

    let (status, failed) = create(app, body.clone()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{failed}");
    let message = failed["message"].as_str().unwrap_or("");
    assert!(message.contains("was created"), "{failed}");
    assert!(
        message.contains("oxyc env update <app> dev-a1 --plane semantic=branch:feat-x"),
        "{failed}"
    );
    assert_eq!(row(&t.db, app, "dev-a1").await, Some(unpinned(set.clone())));
    assert_eq!(create(app, body).await.0, StatusCode::CONFLICT);

    *pinner.pin_down.lock().expect("pin_down") = false;
    let (status, shown) = put_planes(app, "dev-a1", set).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert!(row(&t.db, app, "dev-a1").await.expect("row").is_pinned());
}
