//! The cloud-role upload against an in-process DuckDB standing in for
//! the workspace's tenant. The load itself (`airhouse::import::load`) runs for
//! real; only the credential, the wire and provisioning are not modelled.

use std::sync::Mutex;

use airhouse::import::{ImportSink, load};
use async_trait::async_trait;
use axum::body::Body;
use axum::extract::FromRequest;
use axum::http::Request;

use super::*;

struct DuckSink(Mutex<duckdb::Connection>);

#[async_trait]
impl ImportSink for DuckSink {
    async fn execute(&self, sql: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .execute_batch(sql)
            .map_err(|e| e.to_string())
    }

    async fn count(&self, sql: &str) -> Result<u64, String> {
        self.0
            .lock()
            .unwrap()
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(|e| e.to_string())
    }
}

/// What the stand-in's readiness check answers.
enum Ready {
    Yes,
    NotConfigured,
    ProvisioningFails,
}

/// A workspace whose tenant is a DuckDB database.
struct StandIn {
    sink: DuckSink,
    ready: Ready,
}

impl StandIn {
    fn new(ready: Ready) -> Self {
        Self {
            sink: DuckSink(Mutex::new(duckdb::Connection::open_in_memory().unwrap())),
            ready,
        }
    }

    fn rows(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        let conn = self.sink.0.lock().unwrap();
        let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}\n{e}"));
        let mut rows = stmt.query([]).unwrap_or_else(|e| panic!("{sql}\n{e}"));
        let mut out = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let n = row.as_ref().column_count();
            out.push(
                (0..n)
                    .map(|i| {
                        row.get::<_, duckdb::types::Value>(i)
                            .map(|v| match v {
                                duckdb::types::Value::Null => None,
                                other => Some(format!("{other:?}")),
                            })
                            .unwrap()
                    })
                    .collect(),
            );
        }
        out
    }

    fn count(&self, from: &str) -> i64 {
        self.sink
            .0
            .lock()
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {from}"), [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("FROM {from}: {e}"))
    }
}

#[async_trait]
impl WorkspaceImporter for StandIn {
    async fn ready(&self, workspace_id: Uuid) -> Result<(), ImportError> {
        match self.ready {
            Ready::Yes => Ok(()),
            Ready::NotConfigured => Err(ImportError::NotConfigured),
            Ready::ProvisioningFails => {
                Err(ImportError::Tenant(airhouse::EnsureTenantError::Provision(
                    airhouse::ProvisionerError::WorkspaceNotFound(workspace_id),
                )))
            }
        }
    }

    async fn import(
        &self,
        workspace_id: Uuid,
        file: &ImportFile,
    ) -> Result<ImportOutcome, ImportError> {
        self.ready(workspace_id).await?;
        load(&self.sink, file).await
    }
}

const BOUNDARY: &str = "oxy-airhouse-upload-test";

/// A multipart body of `(field, file name, contents)` parts, as the web sends.
async fn multipart(parts: &[(&str, Option<&str>, &str)]) -> Multipart {
    let mut body = String::new();
    for (name, file_name, contents) in parts {
        body.push_str(&format!("--{BOUNDARY}\r\n"));
        match file_name {
            Some(f) => body.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )),
            None => body.push_str(&format!(
                "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"
            )),
        }
        body.push_str(contents);
        body.push_str("\r\n");
    }
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    let request = Request::builder()
        .method("POST")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    Multipart::from_request(request, &()).await.unwrap()
}

#[test]
fn only_the_local_all_in_one_keeps_data_in_files() {
    assert!(!data_in_airhouse(Role::All));
    for role in [Role::Ide, Role::Serve, Role::Worker] {
        assert!(data_in_airhouse(role), "{}", role.as_str());
    }
}

#[tokio::test]
async fn an_upload_on_a_cloud_role_lands_in_airhouse() {
    let tenant = StandIn::new(Ready::Yes);
    let ws = Uuid::new_v4();
    let body = multipart(&[
        ("subdir", None, ".db"),
        ("file", Some("orders.csv"), "id,amount\n1,10\n2,20\n"),
        ("file", Some("notes.txt"), "not data"),
    ])
    .await;

    let (status, Json(response)) = upload_into_airhouse(&tenant, ws, body).await.unwrap();

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(response.destination, UploadDestination::Airhouse);
    assert_eq!(response.subdir, "", "nothing was written to a directory");
    assert_eq!(response.files, ["orders.csv"]);
    assert_eq!(response.skipped.len(), 1);
    assert_eq!(response.skipped[0].reason, "unsupported_extension");
    assert_eq!(
        response.tables,
        [UploadedTable {
            file: "orders.csv".into(),
            schema: "main".into(),
            table: "orders".into(),
            aliases: vec!["main.orders.csv".into(), "orders.csv".into()],
            rows: 2,
        }]
    );
    assert_eq!(tenant.count("main.orders"), 2);
    assert_eq!(tenant.count("orders.csv"), 2);

    // The same name again is refused, as the file upload refuses it.
    let again = multipart(&[("file", Some("orders.csv"), "id,amount\n3,30\n")]).await;
    let (status, message) = upload_into_airhouse(&tenant, ws, again).await.unwrap_err();
    assert_eq!(status, StatusCode::CONFLICT, "{message}");
    assert!(message.contains("main.orders"), "{message}");
    assert_eq!(
        tenant.count("main.orders"),
        2,
        "the first load is untouched"
    );
}

#[tokio::test]
async fn an_upload_with_no_usable_tenant_is_refused_before_anything_loads() {
    for (ready, expected) in [
        (Ready::NotConfigured, StatusCode::SERVICE_UNAVAILABLE),
        (Ready::ProvisioningFails, StatusCode::BAD_GATEWAY),
    ] {
        let tenant = StandIn::new(ready);
        let body = multipart(&[("file", Some("orders.csv"), "id\n1\n")]).await;
        let (status, message) = upload_into_airhouse(&tenant, Uuid::new_v4(), body)
            .await
            .unwrap_err();
        assert_eq!(status, expected, "{message}");
        assert_eq!(
            tenant.rows(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'main'"
            ),
            Vec::<Vec<Option<String>>>::new()
        );
    }
    let tenant = StandIn::new(Ready::ProvisioningFails);
    let body = multipart(&[("file", Some("orders.csv"), "id\n1\n")]).await;
    let (_, message) = upload_into_airhouse(&tenant, Uuid::new_v4(), body)
        .await
        .unwrap_err();
    assert!(
        message.contains("no Airhouse tenant, and provisioning one failed"),
        "{message}"
    );
}

#[tokio::test]
async fn a_file_named_into_an_app_schema_is_skipped_not_loaded() {
    let tenant = StandIn::new(Ready::Yes);
    let body = multipart(&[
        ("file", Some("app_store_ops.csv"), "a\n1\n"),
        ("file", Some("fine.csv"), "a\n1\n"),
    ])
    .await;
    let (_, Json(response)) = upload_into_airhouse(&tenant, Uuid::new_v4(), body)
        .await
        .unwrap();
    assert_eq!(response.files, ["fine.csv"]);
    assert_eq!(response.skipped[0].name, "app_store_ops.csv");
    assert!(response.skipped[0].reason.contains("reserved"));
}
