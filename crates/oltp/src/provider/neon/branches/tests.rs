//! The branch client over real HTTP, against a stub speaking Neon's shapes.
//!
//! Mocked HTTP only — never Neon. What can break is the wire contract (which
//! branch a password reset lands on, what the restore body names, whether an
//! operation is awaited), and none of that shows without a transport.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::provider::{BranchInit, BranchRequest, NeonProvider, OltpProvider, ProviderError};

// Delete and its guards, in their own file to keep this one readable.
mod guards;

/// One request the stub saw.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    body: Value,
}

#[derive(Default)]
struct Stub {
    seen: Mutex<Vec<Seen>>,
    /// A branch named `oxy-staging` already exists, left by a crashed attempt.
    orphan: bool,
    /// The orphan's endpoints (read-write unless a test says otherwise).
    orphan_endpoints: Option<Value>,
    restore_missing: bool,
    delete_missing: bool,
    /// The orphan was cut from some other branch, not production.
    orphan_foreign_parent: bool,
    /// Neon reports the branch being deleted as protected.
    protected: bool,
    /// Replaces the whole `GET …/branches/{b}` body: a malformed or foreign
    /// description the delete must refuse on.
    described: Option<Value>,
    fail_operation: bool,
}

impl Stub {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn has(&self, method: &str, path: &str) -> Option<Seen> {
        self.seen()
            .into_iter()
            .find(|s| s.method == method && s.path == path)
    }
}

fn ok(v: Value) -> Response {
    (StatusCode::OK, axum::Json(v)).into_response()
}

fn op(id: &str) -> Value {
    json!([{ "id": id, "status": "running" }])
}

async fn handle(State(s): State<Arc<Stub>>, method: Method, uri: Uri, body: Bytes) -> Response {
    let path = uri.path().to_string();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    s.seen.lock().unwrap().push(Seen {
        method: method.to_string(),
        path: path.clone(),
        body,
    });
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    route(&s, method.as_str(), &parts)
}

fn route(s: &Stub, method: &str, parts: &[&str]) -> Response {
    match (method, parts) {
        ("GET", ["projects", _, "operations", _]) => ok(json!({ "operation": {
            "status": if s.fail_operation { "failed" } else { "finished" },
            "error": "branch compute never started"
        }})),
        ("GET", ["projects", _, "branches"]) => {
            let mut branches = vec![json!({ "id": "br-main", "name": "main", "default": true })];
            if s.orphan {
                let parent = if s.orphan_foreign_parent {
                    "br-dev"
                } else {
                    "br-main"
                };
                branches
                    .push(json!({ "id": "br-orphan", "name": "oxy-staging", "parent_id": parent }));
            }
            ok(json!({ "branches": branches }))
        }
        ("POST", ["projects", _, "branches"]) => ok(json!({
            "branch": { "id": "br-stg", "name": "oxy-staging", "parent_id": "br-main" },
            "endpoints": [
                { "host": "ep-stg-ro.neon.tech", "type": "read_only" },
                { "host": "ep-stg.neon.tech", "type": "read_write" }
            ],
            "operations": op("op-create")
        })),
        ("GET", ["projects", _, "branches", _, "endpoints"]) => ok(json!({
            "endpoints": s.orphan_endpoints.clone().unwrap_or_else(
                || json!([{ "host": "ep-orphan.neon.tech", "type": "read_write" }])
            )
        })),
        ("POST", ["projects", _, "endpoints"]) => ok(json!({
            "endpoint": { "host": "ep-new.neon.tech", "type": "read_write" },
            "operations": op("op-endpoint")
        })),
        (
            "POST",
            [
                "projects",
                _,
                "branches",
                b,
                "roles",
                role,
                "reset_password",
            ],
        ) => ok(json!({
            "role": { "name": role, "password": format!("pw-on-{b}") },
            "operations": []
        })),
        ("POST", ["projects", _, "branches", _, "restore"]) if s.restore_missing => {
            StatusCode::NOT_FOUND.into_response()
        }
        ("POST", ["projects", _, "branches", b, "restore"]) => ok(json!({
            "branch": { "id": b, "name": "oxy-staging" },
            "operations": op("op-restore")
        })),
        ("GET", ["projects", _, "branches", _]) if s.delete_missing => {
            StatusCode::NOT_FOUND.into_response()
        }
        ("GET", ["projects", _, "branches", _]) if s.described.is_some() => {
            ok(s.described.clone().unwrap_or_default())
        }
        ("GET", ["projects", _, "branches", b]) => ok(json!({ "branch": {
            "id": b, "name": "x",
            // The root branch has no parent; everything else was cut from it.
            "parent_id": (*b != "br-main").then_some("br-main"),
            "default": *b == "br-main", "protected": s.protected
        }})),
        ("PATCH", ["projects", _, "branches", _]) if s.restore_missing => {
            StatusCode::NOT_FOUND.into_response()
        }
        ("PATCH", ["projects", _, "branches", b]) => ok(json!({
            "branch": { "id": b }, "operations": op("op-patch")
        })),
        ("DELETE", ["projects", _, "branches", _]) if s.delete_missing => {
            StatusCode::NOT_FOUND.into_response()
        }
        ("DELETE", ["projects", _, "branches", b]) => ok(json!({
            "branch": { "id": b }, "operations": op("op-delete")
        })),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve(stub: Arc<Stub>) -> NeonProvider {
    let app = Router::new().fallback(handle).with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    NeonProvider::with_base_url("test-key", "org-oxy", format!("http://{addr}"))
}

fn staging() -> BranchRequest {
    BranchRequest::staging(
        "cold-sky-123",
        "br-main",
        "oxy-staging",
        "neondb",
        "oxy_owner",
    )
}

#[tokio::test]
async fn create_cuts_from_production_on_its_own_endpoint_and_owner_password() {
    let stub = Arc::new(Stub::default());
    let branch = serve(stub.clone())
        .await
        .create_branch(&staging())
        .await
        .expect("create");

    let post = stub
        .has("POST", "/projects/cold-sky-123/branches")
        .expect("a branch was requested");
    assert_eq!(post.body["branch"]["name"], "oxy-staging");
    assert_eq!(
        post.body["branch"]["parent_id"], "br-main",
        "cut from production's head"
    );
    assert_eq!(post.body["endpoints"][0]["type"], "read_write");
    assert!(
        stub.has("GET", "/projects/cold-sky-123/operations/op-create")
            .is_some(),
        "the compute must be awaited before a DSN is handed out"
    );
    // The reset lands on the NEW branch. On the parent it would rotate
    // production's owner and strand every sealed production credential.
    assert!(
        stub.has(
            "POST",
            "/projects/cold-sky-123/branches/br-stg/roles/oxy_owner/reset_password"
        )
        .is_some()
    );
    assert!(
        stub.has(
            "POST",
            "/projects/cold-sky-123/branches/br-main/roles/oxy_owner/reset_password"
        )
        .is_none()
    );
    assert_eq!(branch.id, "br-stg");
    assert_eq!(
        branch.host, "ep-stg.neon.tech",
        "read-write, not the replica"
    );
    assert_eq!(branch.parent_id, "br-main");
    assert_eq!(branch.owner_role.password.as_deref(), Some("pw-on-br-stg"));
}

#[tokio::test]
async fn an_orphaned_branch_is_adopted_not_duplicated() {
    let stub = Arc::new(Stub {
        orphan: true,
        ..Default::default()
    });
    let branch = serve(stub.clone())
        .await
        .create_branch(&staging())
        .await
        .expect("adopt");

    assert!(
        stub.has("POST", "/projects/cold-sky-123/branches")
            .is_none(),
        "an existing branch must not be bought twice"
    );
    assert_eq!(branch.id, "br-orphan");
    assert_eq!(branch.host, "ep-orphan.neon.tech");
    assert_eq!(
        branch.owner_role.password.as_deref(),
        Some("pw-on-br-orphan"),
        "the orphan's password died with the attempt that made it"
    );
}

#[tokio::test]
async fn an_adopted_branch_with_only_a_replica_gets_a_read_write_endpoint() {
    let stub = Arc::new(Stub {
        orphan: true,
        orphan_endpoints: Some(json!([{ "host": "ep-ro.neon.tech", "type": "read_only" }])),
        ..Default::default()
    });
    let branch = serve(stub.clone())
        .await
        .create_branch(&staging())
        .await
        .expect("adopt");

    let post = stub
        .has("POST", "/projects/cold-sky-123/endpoints")
        .expect("an endpoint was created");
    assert_eq!(post.body["endpoint"]["branch_id"], "br-orphan");
    assert_eq!(post.body["endpoint"]["type"], "read_write");
    assert_eq!(branch.host, "ep-new.neon.tech");
}

#[tokio::test]
async fn reset_restores_from_the_parent_and_re_mints_the_owner() {
    let stub = Arc::new(Stub::default());
    let branch = serve(stub.clone())
        .await
        .reset_branch(&staging(), "br-stg")
        .await
        .expect("reset");

    let restore = stub
        .has("POST", "/projects/cold-sky-123/branches/br-stg/restore")
        .expect("restored");
    assert_eq!(restore.body, json!({ "source_branch_id": "br-main" }));
    assert!(
        stub.has("GET", "/projects/cold-sky-123/operations/op-restore")
            .is_some()
    );
    assert!(
        stub.has(
            "POST",
            "/projects/cold-sky-123/branches/br-stg/roles/oxy_owner/reset_password"
        )
        .is_some(),
        "the restore brought production's owner password back with the data"
    );
    assert_eq!(branch.id, "br-stg", "a reset keeps the branch id");
    assert_eq!(branch.owner_role.password.as_deref(), Some("pw-on-br-stg"));
}

#[tokio::test]
async fn resetting_a_vanished_branch_is_branch_not_found() {
    let stub = Arc::new(Stub {
        restore_missing: true,
        ..Default::default()
    });
    let err = serve(stub)
        .await
        .reset_branch(&staging(), "br-gone")
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::BranchNotFound(id) if id == "br-gone"));
}

#[tokio::test]
async fn a_same_named_branch_from_elsewhere_is_not_adopted() {
    let stub = Arc::new(Stub {
        orphan: true,
        orphan_foreign_parent: true,
        ..Default::default()
    });
    let err = serve(stub.clone())
        .await
        .create_branch(&staging())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::BranchParentMismatch { ref parent, .. } if parent == "br-dev"),
        "{err}"
    );
    assert!(
        stub.seen()
            .iter()
            .all(|s| !s.path.ends_with("reset_password")),
        "a branch that is not ours must not have its owner reset"
    );
    assert!(
        stub.has("POST", "/projects/cold-sky-123/branches")
            .is_none()
    );
}

#[tokio::test]
async fn a_failed_operation_fails_the_create() {
    let stub = Arc::new(Stub {
        fail_operation: true,
        ..Default::default()
    });
    let err = serve(stub)
        .await
        .create_branch(&staging())
        .await
        .expect_err("a branch whose compute never started is not a branch");
    assert!(
        err.to_string().contains("branch compute never started"),
        "{err}"
    );
}

#[tokio::test]
async fn create_sends_the_fork_point_expiry_and_init_source() {
    let stub = Arc::new(Stub::default());
    let mut req = staging();
    req.parent_timestamp = Some("2026-11-01T09:00:00Z".parse().unwrap());
    req.expires_at = Some("2026-12-01T09:00:00Z".parse().unwrap());
    req.init = BranchInit::SchemaOnly;
    serve(stub.clone())
        .await
        .create_branch(&req)
        .await
        .expect("create");

    let post = stub
        .has("POST", "/projects/cold-sky-123/branches")
        .expect("a branch was requested");
    let branch = &post.body["branch"];
    assert_eq!(branch["parent_timestamp"], "2026-11-01T09:00:00Z");
    assert_eq!(branch["expires_at"], "2026-12-01T09:00:00Z");
    assert_eq!(branch["init_source"], "schema-only");
    assert_eq!(
        branch["parent_id"], "br-main",
        "still named: the fork point is a time on THIS parent"
    );
}

#[tokio::test]
async fn create_with_defaults_sends_none_of_them() {
    let stub = Arc::new(Stub::default());
    serve(stub.clone())
        .await
        .create_branch(&staging())
        .await
        .expect("create");

    let post = stub
        .has("POST", "/projects/cold-sky-123/branches")
        .expect("a branch was requested");
    let branch = post.body["branch"].as_object().expect("a branch object");
    for key in ["parent_timestamp", "expires_at", "init_source"] {
        assert!(
            !branch.contains_key(key),
            "{key} must be absent: {branch:?}"
        );
    }
}

#[tokio::test]
async fn reset_of_a_schema_only_branch_is_refused_before_any_request() {
    let stub = Arc::new(Stub::default());
    let mut req = staging();
    req.init = BranchInit::SchemaOnly;
    let err = serve(stub.clone())
        .await
        .reset_branch(&req, "br-stg")
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::Api { status: 400, ref message } if message.contains("schema-only")),
        "{err}"
    );
    assert!(
        stub.seen().is_empty(),
        "nothing may reach Neon: {:?}",
        stub.seen()
    );
}

#[tokio::test]
async fn a_branch_is_found_by_its_derived_name_or_not_at_all() {
    let stub = Arc::new(Stub {
        orphan: true,
        ..Default::default()
    });
    let found = serve(stub.clone()).await.find_branch_id(&staging()).await;
    assert_eq!(found.expect("find").as_deref(), Some("br-orphan"));

    let none = serve(Arc::new(Stub::default()))
        .await
        .find_branch_id(&staging())
        .await;
    assert_eq!(none.expect("find"), None, "main is not named oxy-staging");
}

#[tokio::test]
async fn an_expiry_is_patched_onto_the_branch_and_awaited() {
    let stub = Arc::new(Stub::default());
    let at = chrono::DateTime::parse_from_rfc3339("2026-12-02T10:00:00Z")
        .unwrap()
        .to_utc();

    serve(stub.clone())
        .await
        .set_branch_expiry(&staging(), "br-stg", at)
        .await
        .expect("patch");

    let patch = stub
        .has("PATCH", "/projects/cold-sky-123/branches/br-stg")
        .expect("the branch was patched");
    assert_eq!(patch.body["branch"]["expires_at"], "2026-12-02T10:00:00Z");
    assert!(
        stub.has("GET", "/projects/cold-sky-123/operations/op-patch")
            .is_some()
    );
}

#[tokio::test]
async fn an_expiry_on_a_missing_branch_is_not_found_and_on_production_is_refused() {
    let stub = Arc::new(Stub {
        restore_missing: true,
        ..Default::default()
    });
    let provider = serve(stub.clone()).await;
    let at = chrono::Utc::now();

    let gone = provider.set_branch_expiry(&staging(), "br-gone", at).await;
    assert!(
        matches!(gone, Err(ProviderError::BranchNotFound(_))),
        "{gone:?}"
    );

    let main = provider.set_branch_expiry(&staging(), "br-main", at).await;
    assert!(main.is_err(), "production's expiry is never moved");
    assert!(
        stub.has("PATCH", "/projects/cold-sky-123/branches/br-main")
            .is_none()
    );
}
