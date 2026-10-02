//! `POST /api/customer-apps/publish` with the `environment` multipart field
//! (`internal-docs/custom-app-sandboxes.md` §5.2), through the real handler.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Extension, Router, middleware};
use oxy_app::server::api::custom_apps_publish::publish_handler;
use oxy_auth::middleware::{AuthState, auth_middleware};
use oxy_auth::types::AppPublishTokenAuth;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_publish::{app_with_two_sandboxes, bundle, noop};

const BOUNDARY: &str = "sandbox-publish-boundary";

/// The multipart body `oxyc publish` sends: text `fields`, then the bundle.
fn multipart(fields: &[(&str, &str)], tarball: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n\
                 {value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"bundle\"; \
             filename=\"bundle.tar.gz\"\r\nContent-Type: application/gzip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(tarball);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

/// `POST /api/customer-apps/publish` through the real handler, as the
/// signed-in guest — with a publish-token marker when `token`.
async fn publish(slug: &str, fields: &[(&str, &str)], token: bool) -> (StatusCode, String) {
    let workspace = demo_workspace_id().to_string();
    let mut all = vec![("app", slug), ("project", workspace.as_str())];
    all.extend_from_slice(fields);
    let tarball = bundle(
        slug,
        &[noop("noop", json!({ "route": true }))],
        json!({}),
        &[],
    );
    let mut router = Router::new().route("/api/customer-apps/publish", post(publish_handler));
    if token {
        router = router.layer(Extension(AppPublishTokenAuth {
            token_id: Uuid::new_v4(),
            app_id: None,
            machine_identity: None,
        }));
    }
    let router = router.layer(middleware::from_fn_with_state(
        AuthState::built_in(),
        auth_middleware,
    ));
    let request = Request::builder()
        .method("POST")
        .uri("/api/customer-apps/publish")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(&all, &tarball)))
        .expect("request");
    let response = router.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
}

/// The `environment` multipart field selects a sandbox; absent or empty is
/// today's publish. Every publish says which environment it moved and that
/// environment's host. The refusals keep this route's plain-text bodies.
#[tokio::test]
async fn the_environment_field_selects_a_sandbox_and_every_publish_names_its_environment() {
    let t = seeded_tenant().await;
    let slug = "sbx-route";
    app_with_two_sandboxes(&t, slug).await;

    let (status, body) = publish(
        slug,
        &[("build_id", "to-a1"), ("environment", "dev-a1")],
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let published = json_of(&body);
    assert_eq!(published["channel"], "sandbox");
    assert_eq!(published["environment"], "dev-a1");
    assert_eq!(
        published["environment_url"],
        format!("https://dev-a1--local--{slug}.customer-apps-dev.oxygen-hq.com/")
    );
    assert_eq!(published["is_new_app"], false);

    // Absent and empty are the publish this route always made.
    let (status, body) = publish(slug, &[("build_id", "draft-1")], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let draft = json_of(&body);
    assert_eq!(
        (&draft["channel"], &draft["environment"]),
        (&json!("draft"), &json!("staging"))
    );
    assert_eq!(
        draft["environment_url"],
        format!("https://staging--local--{slug}.customer-apps-dev.oxygen-hq.com/")
    );
    let (status, body) = publish(
        slug,
        &[
            ("build_id", "live-1"),
            ("environment", ""),
            ("promote", "true"),
        ],
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let live = json_of(&body);
    assert_eq!(
        (&live["channel"], &live["environment"]),
        (&json!("published"), &json!("production"))
    );
    assert_eq!(
        live["environment_url"],
        format!("https://local--{slug}.customer-apps-dev.oxygen-hq.com/")
    );

    // The refusals, each with its status and a plain-text body.
    for (fields, expected, says) in [
        (
            vec![("environment", "staging")],
            StatusCode::BAD_REQUEST,
            "not a sandbox name",
        ),
        (
            vec![("environment", "dev--x")],
            StatusCode::BAD_REQUEST,
            "not a sandbox name",
        ),
        (
            vec![("environment", "dev-a1"), ("promote", "true")],
            StatusCode::BAD_REQUEST,
            "promote",
        ),
        (
            vec![("environment", "dev-a1"), ("channel", "published")],
            StatusCode::BAD_REQUEST,
            "promote",
        ),
        (
            vec![("environment", "dev-nobody")],
            StatusCode::NOT_FOUND,
            "dev-nobody",
        ),
    ] {
        let mut fields = fields;
        fields.push(("build_id", "refused"));
        let (status, body) = publish(slug, &fields, false).await;
        assert_eq!(status, expected, "{fields:?}: {body}");
        assert!(body.contains(says), "{fields:?}: {body}");
        assert!(
            serde_json::from_str::<Value>(&body).is_err(),
            "a plain-text body: {body}"
        );
    }

    // A publish token cannot publish to a sandbox, whoever minted it.
    let (status, body) = publish(
        slug,
        &[("build_id", "by-token"), ("environment", "dev-a1")],
        true,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // …and still publishes to staging, as it always could.
    let (status, body) = publish(slug, &[("build_id", "by-token")], true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
