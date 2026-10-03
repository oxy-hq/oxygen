//! What the upload tests run on: a real `ProjectFunctionHost` per invocation
//! (no V8, as in `staging_homes_fixture`), an object store that presigns and
//! nothing else, and the calls each test makes.
//!
//! **Why no object store answers here.** `ctx.fetch` sends only HTTPS to a
//! public host, and that is checked before the environment policy is asked —
//! so a stand-in on loopback is refused whatever the policy says, and the two
//! cannot be told apart. Every URL here is therefore signed for an HTTPS
//! endpoint on a name that cannot resolve: a presign is signed offline, a PUT
//! the policy lets through is **sent** and fails on the wire (`fetch failed`),
//! and a held one answers `409` unsent. Nothing needs a network.

use std::sync::Arc;

use oxy_app::server::api::custom_apps_functions::InvocationIdentity;
use oxy_app::server::api::custom_apps_functions::env_policy::EnvPolicy;
use oxy_app::server::api::custom_apps_functions::host::{
    FunctionCapabilities, ProjectFunctionHost, into_arc,
};
use oxy_app::server::api::custom_apps_functions::runtime::FunctionHost;
use oxy_app::server::api::custom_apps_functions::seam::FunctionQueryExecutor;
use oxy_app::server::api::custom_apps_storage::{RetentionPolicy, Silo, get_upload_url};
use oxy_app::server::api::projects::query::DataPlaneQueryExecutor;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::Tenant;
use crate::staging_homes_fixture::{Workspace, duck, workspace};

/// The object store every URL here is signed for (path-style, as any
/// `AWS_ENDPOINT_URL` store is addressed).
pub(crate) const STORE_HOST: &str = "objects.nonprod-uploads.invalid";
pub(crate) const BUCKET: &str = "nonprod-uploads";

/// Storage on the filesystem under `OXY_STATE_DIR`: where `put`, `delete` and
/// the environment cap's listing work with no object store.
fn filesystem_store() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET") };
}

/// Storage on [`STORE_HOST`], which presigns and answers nothing.
fn presigning_store() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET", BUCKET);
        std::env::set_var("AWS_ENDPOINT_URL", format!("https://{STORE_HOST}"));
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_REGION", "us-east-1");
        std::env::set_var("AWS_CONFIG_FILE", "/nonexistent/oxy-nonprod-uploads");
        std::env::set_var(
            "AWS_SHARED_CREDENTIALS_FILE",
            "/nonexistent/oxy-nonprod-uploads",
        );
    }
}

/// One app, and what its invocations' hosts are built over.
pub(crate) struct Rig {
    pub(crate) app_id: Uuid,
    org_id: Uuid,
    project_id: Uuid,
    db: DatabaseConnection,
    workspace: Workspace,
    _state: Option<tempfile::TempDir>,
}

impl Rig {
    /// On `t`'s control plane: `end_of_invocation` writes the held row there,
    /// and production's org quota is read from it. Its `OXY_STATE_DIR` is the
    /// one `test_db` set.
    pub(crate) async fn on(t: &Tenant) -> Self {
        Self::over(t.db.clone(), t.org_id, demo_workspace_id(), None).await
    }

    /// [`Self::on`], for the published app `app_id`.
    pub(crate) async fn of(t: &Tenant, app_id: Uuid) -> Self {
        Self {
            app_id,
            ..Self::on(t).await
        }
    }

    /// With no control plane and a scratch state dir. A held call is
    /// buffered, and its row is written after these tests look; nothing an
    /// environment's host does here reads the database.
    pub(crate) async fn offline() -> Self {
        let state = tempfile::tempdir().expect("a state dir");
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("OXY_STATE_DIR", state.path()) };
        Self::over(
            DatabaseConnection::default(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some(state),
        )
        .await
    }

    async fn over(
        db: DatabaseConnection,
        org_id: Uuid,
        project_id: Uuid,
        state: Option<tempfile::TempDir>,
    ) -> Self {
        filesystem_store();
        Self {
            app_id: Uuid::new_v4(),
            org_id,
            project_id,
            db,
            workspace: workspace(&duck("d")).await,
            _state: state,
        }
    }

    /// One invocation of the app in `environment`: a host of its own, as
    /// every invocation has.
    pub(crate) fn invocation(&self, environment: AppEnvironment) -> Arc<dyn FunctionHost> {
        self.invocation_of(self.app_id, environment)
    }

    /// One invocation of the app `app_id` in `environment`.
    pub(crate) fn invocation_of(
        &self,
        app_id: Uuid,
        environment: AppEnvironment,
    ) -> Arc<dyn FunctionHost> {
        let caps = FunctionCapabilities {
            storage_read: true,
            storage_write: true,
            ..Default::default()
        };
        into_arc(ProjectFunctionHost::new(
            self.workspace.ctx(),
            Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>,
            self.db.clone(),
            Vec::new(),
            self.project_id,
            app_id,
            self.org_id,
            Uuid::nil(),
            "Uploads".into(),
            caps,
            Default::default(),
            oxy_app::server::api::operating_graph::reach::Reach::nowhere(),
            InvocationIdentity {
                invocation_id: Uuid::new_v4(),
                function_name: "upload".into(),
                mode: "route".into(),
                request_id: None,
                app_slug: "uploads".into(),
                user_id: None,
                user_email: None,
            },
            EnvPolicy::for_environment(environment),
        ))
    }
}

/// A sandbox environment, `dev-<handle>`.
pub(crate) fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

/// What `ctx.storage.getUploadUrl` answered.
pub(crate) struct Minted {
    pub(crate) url: String,
    pub(crate) key: String,
}

/// One `ctx.storage.getUploadUrl` on `host`, for a five-byte text upload.
///
/// An environment's cap is measured on the invocation's first write — a
/// listing, which needs a store that answers — so that first write goes to the
/// filesystem store (and is deleted again); the presign after it is offline.
pub(crate) async fn mint(host: &dyn FunctionHost, pathname: &str) -> Minted {
    filesystem_store();
    let warm = host
        .storage(
            "put".into(),
            json!({ "pathname": "warm/seed.txt", "body": "x", "allowOverwrite": true }),
        )
        .await
        .expect("the invocation's first write");
    host.storage("delete".into(), json!({ "key": warm["key"] }))
        .await
        .expect("delete the first write");
    presigning_store();
    let minted = host
        .storage(
            "getUploadUrl".into(),
            json!({ "pathname": pathname, "contentType": "text/plain", "contentLength": 5 }),
        )
        .await;
    filesystem_store();
    let minted = minted.expect("mint an upload URL");
    let text = |field: &str| minted[field].as_str().expect(field).to_string();
    Minted {
        url: text("url"),
        key: text("key"),
    }
}

/// A correctly signed upload URL into `silo`, minted by nobody's invocation:
/// the storage module's own signer, as `getUploadUrl` calls it.
pub(crate) async fn signed_for(silo: &Silo, pathname: &str) -> String {
    presigning_store();
    let signed = get_upload_url(
        silo,
        pathname,
        "text/plain",
        5,
        None,
        &RetentionPolicy::default(),
    )
    .await;
    filesystem_store();
    signed.expect("presign").url
}

/// `ctx.fetch(url, { method, body })` with the upload's five bytes.
pub(crate) async fn send(
    host: &dyn FunctionHost,
    method: &str,
    url: &str,
) -> Result<Value, String> {
    let init = json!({
        "method": method, "body": "hello", "headers": { "content-type": "text/plain" },
    });
    host.fetch(url.to_string(), init).await
}

/// `ctx.fetch(url, { method: "PUT", body })`.
pub(crate) async fn put(host: &dyn FunctionHost, url: &str) -> Result<Value, String> {
    send(host, "PUT", url).await
}

/// The call left the host: sent to a name that cannot resolve, it failed on
/// the wire instead of answering.
#[track_caller]
pub(crate) fn assert_sent(answer: &Result<Value, String>, what: &str) {
    match answer {
        Err(e) => assert!(e.contains("fetch failed"), "{what}: {e}"),
        Ok(answered) => panic!("{what}: was not sent, and answered {answered}"),
    }
}

/// The call was held: `409`, unsent.
#[track_caller]
pub(crate) fn assert_held(answer: &Result<Value, String>, what: &str) {
    match answer {
        Ok(held) => assert_eq!(
            (&held["status"], &held["held"]),
            (&json!(409), &json!(true)),
            "{what}: {held}"
        ),
        Err(e) => panic!("{what}: was sent, not held ({e})"),
    }
}
