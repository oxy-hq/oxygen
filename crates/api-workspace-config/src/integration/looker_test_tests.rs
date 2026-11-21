//! The check against a fake Looker on loopback: each stage the response can
//! name, reached the way production reaches it.

use super::*;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde_json::json;

fn integration(base_url: String) -> LookerIntegration {
    LookerIntegration {
        client_id_var: "LOOKER_CLIENT_ID".into(),
        client_secret_var: "LOOKER_CLIENT_SECRET".into(),
        base_url,
        explores: Vec::new(),
    }
}

fn both() -> Credentials {
    Credentials {
        client_id: Some("id".into()),
        client_secret: Some("s3cret-value".into()),
    }
}

/// A Looker that signs in when `accept` and lists two models.
async fn fake_looker(accept: bool) -> String {
    let login = move || async move {
        if accept {
            (
                StatusCode::OK,
                axum::Json(
                    json!({"access_token": "t", "token_type": "Bearer", "expires_in": 3600}),
                ),
            )
        } else {
            (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"message": "Not found"})),
            )
        }
    };
    let models = || async {
        axum::Json(json!([
            {"name": "ecommerce", "explores": [{"name": "orders"}, {"name": "users"}]},
            {"name": "finance"}
        ]))
    };
    let app = Router::new()
        .route("/api/4.0/login", post(login))
        .route("/api/4.0/lookml_models", get(models));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn credentials_that_sign_in_list_the_models() {
    let base = fake_looker(true).await;
    let result = check("my_looker", &integration(base.clone()), both()).await;
    assert!(result.ok, "{result:?}");
    assert_eq!((result.stage, result.error.as_deref()), (None, None));
    assert_eq!(result.base_url, base);
    assert_eq!(
        result.models,
        vec![
            LookerModelSummary {
                name: "ecommerce".into(),
                explores: 2
            },
            LookerModelSummary {
                name: "finance".into(),
                explores: 0
            },
        ]
    );
}

#[tokio::test]
async fn refused_credentials_stop_at_auth_without_echoing_the_secret() {
    let base = fake_looker(false).await;
    let result = check("my_looker", &integration(base), both()).await;
    assert!(!result.ok);
    assert_eq!(result.stage, Some("auth"));
    let error = result.error.unwrap();
    assert!(!error.contains("s3cret-value"), "{error}");
    assert!(result.models.is_empty());
}

#[tokio::test]
async fn a_missing_secret_stops_at_resolve_naming_the_variable_only() {
    // Nothing listens on this URL: a missing secret must not dial at all.
    let result = check(
        "my_looker",
        &integration("http://127.0.0.1:9".into()),
        Credentials {
            client_id: Some("id".into()),
            client_secret: None,
        },
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.stage, Some("resolve"));
    let error = result.error.unwrap();
    assert!(error.contains("LOOKER_CLIENT_SECRET"), "{error}");
    assert!(error.starts_with("client secret not found"), "{error}");
}

#[tokio::test]
async fn nothing_listening_stops_at_connect() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let result = check(
        "my_looker",
        &integration(format!("http://127.0.0.1:{port}")),
        both(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.stage, Some("connect"));
}

#[test]
fn a_long_error_is_capped() {
    let long = "x".repeat(MAX_ERROR_CHARS + 10);
    let cut = capped(long);
    assert_eq!(cut.chars().count(), MAX_ERROR_CHARS + 1);
    assert!(cut.ends_with('…'));
    assert_eq!(capped("short".into()), "short");
}

/// `oxyc looker test --json` prints this body as is.
#[test]
fn the_response_shape_is_pinned() {
    let body = serde_json::to_value(report(
        "my_looker",
        "https://acme.looker.com:19999",
        Err(("auth", "Authentication failed: Invalid credentials".into())),
        Duration::from_millis(12),
    ))
    .unwrap();
    assert_eq!(
        body,
        json!({
            "ok": false,
            "stage": "auth",
            "error": "Authentication failed: Invalid credentials",
            "integration": "my_looker",
            "base_url": "https://acme.looker.com:19999",
            "models": [],
            "elapsed_ms": 12
        })
    );
}
