//! The messages plane's `capture` mode, through the real serve route and the
//! console's routes: a sandbox created with `messages=capture` renders each
//! `ctx.email.send` and `ctx.notify`, sends nothing (SES is a local mock that
//! must stay silent), and keeps the message in its outbox, which `GET
//! …/environments/{name}/outbox` reads newest first. Production has no outbox
//! (`409 production_has_no_planes`), and a capture counts against the silo's
//! storage cap and is refused, not sent, past it.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant};
use crate::sandbox_isolation::{app_row, publish_into_sandbox};
use crate::sandbox_routes::send;
use crate::staging_function_homes::{EMAIL_JS, call, mock_ses, publish_with_env, ses_sends};
use crate::staging_functions::{data, make_guest_staff, production_host, sandbox_host};

const APP: &str = "outbox-app";

const NOTIFY_JS: &str = r#"
export default async (req, ctx) =>
  Response.json(await ctx.notify({
    to: { userId: "00000000-0000-0000-0000-0000000000aa" },
    kind: "shift_swap", title: "Swap requested", body: "Dana asked to swap",
  }));
"#;

/// Each call's outcome as a value, so a refusal is read rather than thrown.
const TRY_EMAIL_JS: &str = r#"
export default async (req, ctx) => {
  try {
    return Response.json(await ctx.email.send({
      to: ["cfo@customer.example"], subject: "JE posted", text: "posted",
    }));
  } catch (e) {
    return Response.json({ error: String(e && e.message ? e.message : e) });
  }
};
"#;

fn functions() -> [FunctionSpec; 3] {
    let email = |name, js| FunctionSpec {
        name,
        manifest: json!({ "route": true, "email": { "send": true } }),
        js,
    };
    [
        email("send", EMAIL_JS),
        email("try-send", TRY_EMAIL_JS),
        FunctionSpec {
            name: "notify",
            manifest: json!({
                "route": true,
                "notify": { "kinds": { "shift_swap": {} } },
            }),
            js: NOTIFY_JS,
        },
    ]
}

/// The app published and live, the guest made staff, and a sandbox `dev-a1`
/// created with `planes` and serving the same functions.
async fn capturing_sandbox(t: &Tenant, planes: Value) -> Uuid {
    let published = publish_with_env(t, APP, &functions(), json!({}))
        .await
        .expect("publish the app");
    make_guest_staff();
    // SAFETY: nextest runs each test in its own process; set before any
    // console request. Storage is the filesystem store `test_db` points
    // `OXY_STATE_DIR` at: CI sets an S3 bucket, and `mock_ses` points every
    // AWS client, S3 included, at the SES mock.
    unsafe {
        std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com");
        std::env::remove_var("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
    }
    let app = published.app_id;
    let (status, created) = send(
        "POST",
        &format!("{app}/environments"),
        Some(json!({ "name": "dev-a1", "planes": planes })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let row = app_row(t, app).await;
    publish_into_sandbox(t, &row, "a1", "box-1", &functions(), json!({})).await;
    app
}

async fn outbox(app: Uuid, query: &str) -> (StatusCode, Value) {
    send(
        "GET",
        &format!("{app}/environments/dev-a1/outbox{query}"),
        None,
    )
    .await
}

/// An email and a notification are kept, nothing is sent, and the outbox
/// answers newest first with the fields a tester needs.
#[tokio::test]
async fn a_capturing_sandbox_keeps_its_messages_and_sends_nothing() {
    let ses = mock_ses().await;
    let t = seeded_tenant().await;
    let app = capturing_sandbox(&t, json!({ "messages": "capture" })).await;
    let host = sandbox_host(&t, APP, "a1");

    let (status, empty) = outbox(app, "").await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty, json!({ "messages": [], "has_more": false }));

    let sent = call(&t, APP, "send", &host, json!({})).await;
    let got = data(&sent);
    assert_eq!(got["captured"], true, "{got}");
    assert_eq!(got["environment"], "dev-a1");
    assert_eq!(got["deliveredTo"], json!([]));
    let key = got["outboxKey"].as_str().expect("the object's key");
    assert!(
        key.starts_with(&format!("customer-app-storage/{app}~dev-a1/_outbox/")),
        "the sandbox's own silo: {key}"
    );
    let told = call(&t, APP, "notify", &host, json!({})).await;
    assert_eq!(data(&told)["captured"], true, "{}", told.raw);

    assert!(
        ses_sends(&ses).await.is_empty(),
        "capture sends nothing, to nobody"
    );

    let (status, page) = outbox(app, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["has_more"], false);
    let messages = page["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "{page}");
    assert_eq!(messages[0]["kind"], "notify", "newest first");
    assert_eq!(messages[0]["title"], "Swap requested");
    assert_eq!(messages[0]["body"], "Dana asked to swap");
    assert_eq!(messages[0]["function"], "notify");
    assert_eq!(messages[1]["kind"], "email");
    assert_eq!(messages[1]["subject"], "JE posted");
    assert_eq!(messages[1]["body"], "posted");
    assert_eq!(messages[1]["function"], "send");
    assert_eq!(
        messages[1]["to"],
        json!(["cfo@customer.example"]),
        "who the call named: nobody was reached"
    );
    assert_eq!(messages[1]["cc"], json!(["controller@customer.example"]));
    assert!(messages[1]["invocation_id"].as_str().is_some());

    let (_, one) = outbox(app, "?limit=1").await;
    assert_eq!(one["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(one["has_more"], true, "an older message exists");
}

/// `limit` is a whole number of at least 1; production has no outbox; a
/// sandbox that does not exist is not found.
#[tokio::test]
async fn the_route_refuses_a_bad_limit_production_and_a_missing_sandbox() {
    let t = seeded_tenant().await;
    let app = capturing_sandbox(&t, json!({ "messages": "capture" })).await;
    for bad in ["?limit=0", "?limit=-1", "?limit=many"] {
        let (status, refused) = outbox(app, bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {refused}");
        assert_eq!(refused["error"], "invalid_limit", "{bad}");
    }
    let (status, big) = outbox(app, "?limit=100000").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a limit above the maximum is the maximum: {big}"
    );

    let (status, refused) = send(
        "GET",
        &format!("{app}/environments/production/outbox"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "production_has_no_planes");

    let (status, missing) = send("GET", &format!("{app}/environments/dev-zz/outbox"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["error"], "environment_not_found");
}

/// A sandbox that does not capture sends to the invoker as before and keeps
/// nothing: `invoker` stays a mode, and the outbox is capture's alone.
#[tokio::test]
async fn an_invoker_sandbox_keeps_no_outbox() {
    let ses = mock_ses().await;
    let t = seeded_tenant().await;
    let app = capturing_sandbox(&t, json!({})).await;
    let host = sandbox_host(&t, APP, "a1");
    let sent = call(&t, APP, "send", &host, json!({})).await;
    assert_eq!(data(&sent)["messageId"], "m-1", "{}", sent.raw);
    assert_eq!(ses_sends(&ses).await.len(), 1, "the invoker's mail is sent");
    let (_, page) = outbox(app, "").await;
    assert_eq!(page["messages"], json!([]));
}

/// Production never captures: its mail goes where it was addressed, whatever
/// a sandbox's planes say.
#[tokio::test]
async fn production_still_sends() {
    let ses = mock_ses().await;
    let t = seeded_tenant().await;
    capturing_sandbox(&t, json!({ "messages": "capture" })).await;
    let sent = call(&t, APP, "send", &production_host(&t, APP), json!({})).await;
    assert_eq!(data(&sent)["messageId"], "m-1", "{}", sent.raw);
    let sends = ses_sends(&ses).await;
    assert_eq!(sends.len(), 1);
    assert!(sends[0].0.contains(&"cfo@customer.example".to_string()));
}

/// A capture is a write into the sandbox's silo and answers to its cap: past
/// it the call fails the way a refused `ctx.storage.put` does, and nothing is
/// sent in its place.
#[tokio::test]
async fn a_capture_past_the_silos_cap_fails_and_sends_nothing() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES", "16") };
    let ses = mock_ses().await;
    let t = seeded_tenant().await;
    let app = capturing_sandbox(&t, json!({ "messages": "capture" })).await;
    let host = sandbox_host(&t, APP, "a1");

    let attempt = call(&t, APP, "try-send", &host, json!({})).await;
    let refused = data(&attempt)["error"].as_str().unwrap_or_default();
    assert!(
        refused.contains("environment's storage is full"),
        "the storage put's words: {}",
        attempt.raw
    );
    assert!(ses_sends(&ses).await.is_empty(), "never sent instead");
    let (_, page) = outbox(app, "").await;
    assert_eq!(page["messages"], json!([]), "nothing was kept either");
}
