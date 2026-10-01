//! `AirhouseTokenBroker::mint_for_preview` against a wiremock Admin API. A
//! preview Writer asks for exactly its schemas, and is guarded twice: an
//! Airhouse whose `GET /admin/v1/capabilities` does not report
//! `mint.write_schemas` (older than 0.1.49; they answer 404) gets no Writer
//! mint at all, and a Writer that comes back without the exact echo is refused
//! and revoked rather than handed out.
//!
//! Run with: `cargo nextest run -p oxy-app --test airhouse -E 'test(airhouse_broker_preview)'`

use std::time::Duration;

use airhouse::preview_sql::PreviewNamespace;
use airhouse::{BrokerError, UserRole};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::airhouse_broker::{
    make_broker, mint_response, seed_tenant_with_sa, seed_workspace, set_test_encryption_key,
    test_db,
};

const TTL: Duration = Duration::from_secs(900);

/// Mount a mint that checks the request asks for `schema` under the preview's
/// subject, and answers `username` with `echo` as its `write_schemas`.
async fn mount_mint(
    server: &MockServer,
    tenant: &'static str,
    ns: &PreviewNamespace,
    schema: &str,
    echo: Option<Value>,
    times: u64,
) {
    let (asked, key) = (schema.to_string(), ns.key().to_string());
    Mock::given(method("POST"))
        .and(path(format!("/admin/v1/tenants/{tenant}/tokens")))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["write_schemas"], json!([asked]), "{body}");
            let subject = body["subject"].as_str().unwrap_or_default();
            assert!(subject.ends_with(&format!(":preview:{key}")), "{subject}");
            let mut cred = mint_response(tenant, "writer", 900);
            cred["username"] = "eph_preview".into();
            if let Some(echo) = &echo {
                cred["write_schemas"] = echo.clone();
            }
            ResponseTemplate::new(201).set_body_json(cred)
        })
        .expect(times)
        .mount(server)
        .await;
}

/// `GET /admin/v1/capabilities`, asked with the admin token, `times` times.
async fn mount_capabilities(server: &MockServer, write_schemas: bool, times: u64) {
    Mock::given(method("GET"))
        .and(path("/admin/v1/capabilities"))
        .and(header("authorization", "Bearer tok"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"mint.write_schemas": write_schemas})),
        )
        .expect(times)
        .mount(server)
        .await;
}

async fn mount_revoke(server: &MockServer, tenant: &str, times: u64) {
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/admin/v1/tenants/{tenant}/tokens/eph_preview"
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn an_unscoped_preview_writer_is_refused_and_revoked() {
    const TENANT: &str = "broker-preview-old";
    set_test_encryption_key();
    let db = test_db().await;
    let ws = seed_workspace(&db, TENANT).await;
    seed_tenant_with_sa(&db, ws, TENANT).await;
    let ns = PreviewNamespace::for_branch(ws, "feat/je-v2");
    let schema = ns.schema_for("toast_pos").unwrap();

    let server = MockServer::start().await;
    mount_capabilities(&server, true, 1).await;
    mount_mint(&server, TENANT, &ns, &schema, None, 1).await;
    mount_revoke(&server, TENANT, 1).await;

    let err = make_broker(&server)
        .mint_for_preview(ws, &ns, &[schema], UserRole::Writer, TTL)
        .await
        .expect_err("an unscoped Writer was handed out");
    assert!(
        matches!(&err, BrokerError::UnscopedWriter { echoed: None, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn a_scoped_preview_writer_is_handed_out_and_cached() {
    const TENANT: &str = "broker-preview-new";
    set_test_encryption_key();
    let db = test_db().await;
    let ws = seed_workspace(&db, TENANT).await;
    seed_tenant_with_sa(&db, ws, TENANT).await;
    let ns = PreviewNamespace::for_branch(ws, "feat/je-v2");
    let schema = ns.schema_for("toast_pos").unwrap();

    let server = MockServer::start().await;
    mount_capabilities(&server, true, 1).await;
    mount_mint(&server, TENANT, &ns, &schema, Some(json!([schema])), 1).await;
    mount_revoke(&server, TENANT, 0).await;

    let broker = make_broker(&server);
    for _ in 0..2 {
        let cred = broker
            .mint_for_preview(
                ws,
                &ns,
                std::slice::from_ref(&schema),
                UserRole::Writer,
                TTL,
            )
            .await
            .expect("a Writer Airhouse confined as asked");
        assert_eq!(cred.write_schemas, Some(vec![schema.clone()]));
    }
}

/// An Airhouse that cannot scope Writers — it says so, or it predates the
/// capabilities endpoint and answers 404 — is asked once and gets no preview
/// Writer mint at all.
#[tokio::test]
async fn an_airhouse_that_cannot_scope_writers_gets_no_preview_writer_mint() {
    const TENANT: &str = "broker-preview-nocap";
    set_test_encryption_key();
    let db = test_db().await;
    let ws = seed_workspace(&db, TENANT).await;
    seed_tenant_with_sa(&db, ws, TENANT).await;
    let ns = PreviewNamespace::for_branch(ws, "feat/je-v2");
    let schema = ns.schema_for("toast_pos").unwrap();

    for says in [Some(false), None] {
        let server = MockServer::start().await;
        if let Some(supported) = says {
            mount_capabilities(&server, supported, 1).await;
        }
        mount_mint(&server, TENANT, &ns, &schema, Some(json!([schema])), 0).await;
        let broker = make_broker(&server);
        for _ in 0..2 {
            let err = broker
                .mint_for_preview(
                    ws,
                    &ns,
                    std::slice::from_ref(&schema),
                    UserRole::Writer,
                    TTL,
                )
                .await
                .expect_err("a Writer minted where Airhouse cannot scope one");
            assert!(
                matches!(err, BrokerError::ScopedWritersUnsupported),
                "{says:?}: {err}"
            );
        }
    }
}
