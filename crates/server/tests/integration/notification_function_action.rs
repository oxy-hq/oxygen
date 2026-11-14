//! A function action run for real: an app published with a function and a
//! catalogue action pointing at it, a notification sent to a member, and the
//! action route called with the member's own session — the way the web app
//! sends it (`Authorization: <jwt>`), not a fixture's guest.
//!
//! Each piece is the production one: `publish()` into the filesystem build
//! store, `actions::run` resolving the live build's catalogue, the function
//! route (`handle_function_request`) authenticating the forwarded credential,
//! the V8 isolate running the bundled module, the invocation row and its
//! idempotency key. The result asserted is one only the isolate could have
//! computed — the caller's id, the notification's, and a random number that a
//! second run would not repeat.

use std::io::Write;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use chrono::Utc;
use entity::{app_function_invocations, org_members, organizations, users};
use flate2::Compression;
use flate2::write::GzEncoder;
use oxy_api_custom_apps::custom_apps_publish::{OrgRef, PublishInput, publish};
use oxy_api_custom_apps::seed;
use oxy_api_notifications::actions;
use oxy_app_core::AppState;
use oxy_app_core::serve_mode::ServeMode;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AuthenticatedUser, Identity};
use oxy_auth::user::{LOCAL_GUEST_EMAIL, UserService};
use sea_orm::QueryFilter;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{Schema, demo_workspace_id, examples_path, test_db_with};
use crate::notification_fixture::{Signal, join, read_at, seed_user, signal};

const APP: &str = "alarms";
const FUNCTION: &str = "ack-alarm";
/// The admin host: production, as the web app calls from.
const HOST: &str = "app.oxygen-hq.com";

const ACK_ALARM_JS: &str = r#"
export default async (req, ctx) => {
  const { notificationId, action, link } = JSON.parse(req.body);
  return Response.json({ acked: notificationId, action, link, by: ctx.user.id, run: Math.random() });
};
"#;

fn manifest() -> Value {
    json!({
        "schemaVersion": 2,
        "slug": APP,
        "functions": { FUNCTION: { "route": true } },
        "notifications": {
            "categories": { "operations": { "label": "Store operations" } },
            "kinds": {
                "temp_alarm": {
                    "category": "operations",
                    "label": "Temperature alarm",
                    "actions": [{ "id": "ack", "label": "Acknowledge", "function": FUNCTION }]
                }
            }
        }
    })
}

/// The tar.gz `oxyc publish` uploads: the page, the manifest, the function.
fn bundle() -> Vec<u8> {
    let manifest = manifest().to_string();
    let files: [(&str, &[u8]); 3] = [
        (
            "index.html",
            b"<!doctype html><html><head><title>alarms</title></head><body></body></html>",
        ),
        ("oxy-app.json", manifest.as_bytes()),
        ("functions/ack-alarm.js", ACK_ALARM_JS.as_bytes()),
    ];
    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, bytes)
            .expect("append file");
    }
    let mut gz = tar.into_inner().expect("finish tar");
    gz.flush().expect("flush gzip");
    gz.finish().expect("finish gzip")
}

/// `seed_demo`'s Local org, with the guest as its Owner to publish.
async fn local_org(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let org = organizations::Entity::find()
        .filter(organizations::Column::Slug.eq("local"))
        .one(db)
        .await
        .expect("query org")
        .expect("seed_demo creates the local org")
        .id;
    let guest = UserService::get_or_create_user(&Identity {
        user_id: None,
        email: LOCAL_GUEST_EMAIL.to_string(),
        name: Some("Local User".to_string()),
        picture: None,
    })
    .await
    .expect("guest user");
    let now = Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(guest.id),
        role: Set(org_members::OrgRole::Owner),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .expect("guest owner");
    (org, guest.id)
}

async fn publish_app(org: Uuid, guest: Uuid) -> Uuid {
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(org)),
        app_slug: APP.to_string(),
        project_id: demo_workspace_id(),
        branch: None,
        build_id: "alarms-1".to_string(),
        name: None,
        promote: true,
        tarball: bundle(),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(guest),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        publisher: Some(oxy_server_authz::Caller::without_credential(
            guest,
            LOCAL_GUEST_EMAIL,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    })
    .await
    .expect("an app with a function and a catalogue action publishes")
    .app_id
}

/// A person with a browser session, as the web app holds it.
async fn person(db: &DatabaseConnection) -> (AuthenticatedUser, String) {
    let id = seed_user(db).await;
    let row = users::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("query user")
        .expect("seeded");
    let jwt = oxy_app::surface::session::create_auth_token(row.clone())
        .await
        .expect("mint a session");
    let user = AuthenticatedUser {
        id,
        email: row.email,
        name: row.name,
        picture: None,
        status: row.status,
        credential: None,
    };
    (user, jwt)
}

fn state() -> AppState {
    AppState {
        enterprise: true,
        internal: false,
        mode: ServeMode::Cloud,
        observability: None,
        startup_cwd: std::path::PathBuf::new(),
        preagg_cache: None,
        preagg_renewal_threshold_secs: None,
        agentic_state: None,
        semantic_layer_cache: oxy_app_core::workspace_cache::new_semantic_layer_cache(),
        semantic_engine_cache: oxy_app_core::workspace_cache::new_semantic_engine_cache(),
    }
}

/// `POST /api/notifications/{id}/actions/ack` as `user`, presenting `credential`
/// in `Authorization` the way the web app's axios does (no scheme).
async fn tap(user: &AuthenticatedUser, id: Uuid, credential: &str) -> (StatusCode, Value) {
    let mut headers = HeaderMap::new();
    headers.insert("host", HeaderValue::from_static(HOST));
    headers.insert(
        "authorization",
        HeaderValue::from_str(credential).expect("header"),
    );
    let response: Response = actions::run(
        State(state()),
        // The runtime the composition root hands `oxy-app`: the real one, over the
        // same data-plane executor production uses.
        Some(axum::Extension(std::sync::Arc::new(
            oxy_api_custom_apps::function_runtime::Runtime::new(std::sync::Arc::new(
                oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor,
            )),
        )
            as std::sync::Arc<
                dyn oxy_app_core::function_seam::FunctionRuntime,
            >)),
        AuthenticatedUserExtractor(user.clone()),
        Path((id, "ack".to_string())),
        headers,
    )
    .await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn invocations(db: &DatabaseConnection, app: Uuid) -> Vec<app_function_invocations::Model> {
    app_function_invocations::Entity::find()
        .filter(app_function_invocations::Column::AppId.eq(app))
        .filter(app_function_invocations::Column::FunctionName.eq(FUNCTION))
        .all(db)
        .await
        .expect("query invocations")
}

#[tokio::test]
async fn notification_categories_a_function_action_runs_as_the_tapper_once() {
    let db = test_db_with(Schema::All).await;
    seed::seed_demo(Some(examples_path()))
        .await
        .expect("seed_demo");
    let (org, guest) = local_org(&db).await;
    let app = publish_app(org, guest).await;
    // Sessions are checked from here on: the zero-config guest fallback would
    // let any credential through as the guest.
    oxy_auth::built_in::set_auth_configured(true);
    oxy_auth::token::cache::clear();

    let (member, session) = person(&db).await;
    join(&db, org, member.id).await;
    let alarm = |user: Uuid| Signal {
        org,
        user,
        app: Some(app),
        kind: "temp_alarm",
        thread: None,
        ago: 1,
        read: false,
    };
    let first = signal(&db, alarm(member.id)).await;

    let (status, answer) = tap(&member, first, &session).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["ok"], true, "{answer}");
    let result = &answer["result"];
    assert_eq!(result["acked"], first.to_string(), "{answer}");
    assert_eq!(result["action"], "ack");
    assert_eq!(result["link"], Value::Null);
    assert_eq!(
        result["by"],
        member.id.to_string(),
        "the function ran as the person who tapped"
    );
    assert!(result["run"].is_number(), "{answer}");
    assert!(
        read_at(&db, first).await.is_some(),
        "a success marks it read"
    );

    // A second tap replays the first answer; the function does not run again.
    let (status, again) = tap(&member, first, &session).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["result"], *result, "the stored result, not a new run");
    let rows = invocations(&db, app).await;
    let seen: Vec<_> = rows
        .iter()
        .map(|r| (r.status.as_str(), r.user_id, r.idempotency_key.clone()))
        .collect();
    assert_eq!(
        seen,
        vec![(
            "success",
            Some(member.id),
            Some(format!("notification-action:{first}:ack"))
        )]
    );

    // A credential the function route will not take is the action refused,
    // never a 401 the web app would sign the person out on.
    let second = signal(&db, alarm(member.id)).await;
    let (status, refused) = tap(&member, second, "not-a-session").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["error"], "FunctionRefused", "{refused}");
    assert!(
        read_at(&db, second).await.is_none(),
        "a refusal leaves it unread"
    );

    // Someone outside the org holding a notification from the app: the
    // route's own 403 (no access), as the same refusal.
    let (outsider, theirs) = person(&db).await;
    let held = signal(&db, alarm(outsider.id)).await;
    let (status, refused) = tap(&outsider, held, &theirs).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["error"], "FunctionRefused", "{refused}");
    assert!(read_at(&db, held).await.is_none());
    assert_eq!(
        invocations(&db, app).await.len(),
        1,
        "neither refusal ran it"
    );
}
