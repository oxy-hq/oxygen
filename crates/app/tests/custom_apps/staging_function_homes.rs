//! Phase 5a of the previews plan: two staging writes gain an **isolated
//! home** instead of being held (environments design §4.2). Secrets are
//! `staging_function_secrets`'.
//!
//! - `ctx.storage` works in the sibling silo `customer-app-storage/<id>~staging/`,
//!   reading production's same key read-only; `delete` and `copy` never reach
//!   production's silo, `list` is staging's alone, and without `allowOverwrite`
//!   production's key reads as existing;
//! - `ctx.email.send` reaches the invoking user only, replies included,
//!   subject `[staging] …`.
//!
//! Every call goes through the real serve route on a `staging--` or production
//! host (`staging_functions`). Storage is the filesystem store `test_db` points
//! `OXY_STATE_DIR` at; SES is a local mock.

use axum::http::StatusCode;
use oxy_app::server::api::custom_apps_publish::{
    OrgRef, PublishError, PublishInput, PublishResult, publish,
};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use serde_json::{Value, json};

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FnCall, FunctionSpec, Tenant, call_function_with, seeded_tenant,
};
use crate::custom_apps_publish_function_artifacts::tar_gz;
use crate::staging_functions::{data, make_guest_staff, production_host, staging_host};

const INDEX_HTML: &[u8] =
    b"<!doctype html><html><head><title>homes</title></head><body></body></html>";

/// A promoted build of `functions` whose `oxy-app.json` carries `env`, so
/// production and staging serve the same code.
pub(crate) async fn publish_with_env(
    t: &Tenant,
    slug: &str,
    functions: &[FunctionSpec],
    env: Value,
) -> Result<PublishResult, PublishError> {
    publish_env_build(t, slug, functions, env, "homes-1", true).await
}

/// A build `build_id` of `functions` with `env` in its manifest: promoted to
/// production when `promote`, else served by staging alone.
pub(crate) async fn publish_env_build(
    t: &Tenant,
    slug: &str,
    functions: &[FunctionSpec],
    env: Value,
    build_id: &str,
    promote: bool,
) -> Result<PublishResult, PublishError> {
    let declared: serde_json::Map<String, Value> = functions
        .iter()
        .map(|f| (f.name.to_string(), f.manifest.clone()))
        .collect();
    let manifest =
        json!({ "schemaVersion": 2, "slug": slug, "functions": declared, "env": env }).to_string();
    let artifacts: Vec<(String, &[u8])> = functions
        .iter()
        .map(|f| (format!("functions/{}.js", f.name), f.js.as_bytes()))
        .collect();
    let mut files: Vec<(&str, &[u8])> = vec![
        ("index.html", INDEX_HTML),
        ("oxy-app.json", manifest.as_bytes()),
    ];
    files.extend(artifacts.iter().map(|(p, js)| (p.as_str(), *js)));
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: demo_workspace_id(),
        branch: None,
        build_id: build_id.to_string(),
        name: None,
        promote,
        tarball: tar_gz(&files),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    })
    .await
}

pub(crate) async fn call(t: &Tenant, app: &str, name: &str, host: &str, body: Value) -> FnCall {
    let call = call_function_with(&t.org_slug, app, name, body, &[("host", host)]).await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw);
    call
}

// ── Storage ──────────────────────────────────────────────────────────────────

const WRITE_REPORT_JS: &str = r#"
export default async (req, ctx) =>
  Response.json(await ctx.storage.put("generated/report.csv", "prod", { allowOverwrite: true }));
"#;

/// Staging, holding production's key: read it, write the same pathname,
/// delete it, copy it, list.
const STAGING_STORAGE_JS: &str = r#"
export default async (req, ctx) => {
  const { prodKey } = JSON.parse(req.body);
  const body = async (k) => { const o = await ctx.storage.get(k); return o ? o.body : null; };
  const out = { channel: ctx.channel };
  out.before = await body(prodKey);
  try { await ctx.storage.put("generated/report.csv", "stg"); out.fresh = "written"; }
  catch (e) { out.fresh = String(e && e.message ? e.message : e); }
  out.put = (await ctx.storage.put("generated/report.csv", "stg", { allowOverwrite: true })).key;
  out.after = await body(prodKey);
  out.deleted = (await ctx.storage.delete(prodKey)).deleted;
  out.afterDelete = await body(prodKey);
  out.copy = (await ctx.storage.copy(prodKey, "archive/report.csv")).key;
  out.list = (await ctx.storage.list()).objects.map((o) => o.key).sort();
  return Response.json(out);
};
"#;

const LIST_JS: &str = r#"
export default async (req, ctx) => {
  const { key } = JSON.parse(req.body);
  const o = await ctx.storage.get(key);
  return Response.json({ list: (await ctx.storage.list()).objects.map((o) => o.key), body: o ? o.body : null });
};
"#;

fn storage_fn(name: &'static str, js: &'static str) -> FunctionSpec {
    FunctionSpec {
        name,
        manifest: json!({ "route": true, "storage": { "read": true, "write": true } }),
        js,
    }
}

#[tokio::test]
async fn a_staging_function_works_in_its_own_silo_and_never_touches_productions() {
    let t = seeded_tenant().await;
    let app = "stg-silo";
    let functions = [
        storage_fn("write", WRITE_REPORT_JS),
        storage_fn("probe", STAGING_STORAGE_JS),
        storage_fn("look", LIST_JS),
    ];
    let published = publish_with_env(&t, app, &functions, json!({}))
        .await
        .unwrap();
    make_guest_staff();
    let (prod_host, stg_host) = (production_host(&t, app), staging_host(&t, app));

    let written = call(&t, app, "write", &prod_host, json!({})).await;
    let prod_key = data(&written)["key"].as_str().unwrap().to_string();
    let prod_silo = format!("customer-app-storage/{}/", published.app_id);
    let stg_silo = format!("customer-app-storage/{}~staging/", published.app_id);
    assert!(prod_key.starts_with(&prod_silo), "{prod_key}");

    let staged = call(&t, app, "probe", &stg_host, json!({ "prodKey": prod_key })).await;
    let got = data(&staged);
    assert_eq!(got["channel"], "staging");
    assert_eq!(got["before"], "prod", "staging reads production's object");
    assert!(
        got["fresh"]
            .as_str()
            .unwrap()
            .contains("already exists in production"),
        "without allowOverwrite production's key reads as existing: {}",
        got["fresh"]
    );
    assert_eq!(got["put"], format!("{stg_silo}generated/report.csv"));
    assert_eq!(got["after"], "stg", "its own copy shadows production's");
    assert_eq!(got["deleted"], 1);
    assert_eq!(
        got["afterDelete"], "prod",
        "the delete removed staging's copy only"
    );
    assert_eq!(got["copy"], format!("{stg_silo}archive/report.csv"));
    assert_eq!(
        got["list"],
        json!([format!("{stg_silo}archive/report.csv")]),
        "a staging listing is its own silo alone"
    );

    let prod = call(&t, app, "look", &prod_host, json!({ "key": prod_key })).await;
    assert_eq!(
        data(&prod),
        &json!({ "list": [prod_key], "body": "prod" }),
        "production's silo is exactly what production wrote"
    );
}

// ── Email ────────────────────────────────────────────────────────────────────

const EMAIL_JS: &str = r#"
export default async (req, ctx) =>
  Response.json(await ctx.email.send({
    to: ["cfo@customer.example"], cc: "controller@customer.example", bcc: ["audit@customer.example"],
    replyTo: "ap@customer.example", subject: "JE posted", text: "posted",
  }));
"#;

/// SES answered by a local mock, so the envelope SES would deliver to is read
/// back from the request itself.
async fn mock_ses() -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(method("POST"))
        .and(path("/v2/email/outbound-emails"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(json!({ "MessageId": "m-1" })),
        )
        .mount(&server)
        .await;
    // SAFETY: nextest runs each test in its own process; set before the SES
    // client (a process-wide once-cell) is first built.
    unsafe {
        std::env::set_var("OXY_APP_EMAIL_LOCAL_TEST", "0");
        std::env::set_var("AWS_ENDPOINT_URL", server.uri());
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_REGION", "us-east-1");
        std::env::set_var("AWS_CONFIG_FILE", "/nonexistent/oxy-homes-test");
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", "/nonexistent/oxy-homes-test");
    }
    server
}

/// Every address SES was asked to deliver to, and each raw message.
///
/// Only SES's own requests: `AWS_ENDPOINT_URL` is every AWS client's endpoint,
/// so where `OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET` is set (CI) the seeded apps'
/// S3 storage uploads land on this mock too, raw file bytes as their bodies.
async fn ses_sends(server: &wiremock::MockServer) -> Vec<(Vec<String>, String)> {
    use base64::Engine as _;
    let requests = server.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .filter(|r| r.url.path() == "/v2/email/outbound-emails")
        .map(|r| {
            let body: Value = serde_json::from_slice(&r.body).expect("SES JSON");
            let dest = &body["Destination"];
            let mut to: Vec<String> = ["ToAddresses", "CcAddresses", "BccAddresses"]
                .iter()
                .flat_map(|f| dest[*f].as_array().cloned().unwrap_or_default())
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            to.sort();
            let raw = body["Content"]["Raw"]["Data"].as_str().unwrap_or_default();
            let raw = base64::engine::general_purpose::STANDARD
                .decode(raw)
                .expect("base64 message");
            (to, String::from_utf8_lossy(&raw).into_owned())
        })
        .collect()
}

#[tokio::test]
async fn a_staging_email_reaches_the_invoking_user_alone() {
    let ses = mock_ses().await;
    let t = seeded_tenant().await;
    let app = "stg-mail";
    let send = FunctionSpec {
        name: "send",
        manifest: json!({ "route": true, "email": { "send": true } }),
        js: EMAIL_JS,
    };
    publish_with_env(&t, app, std::slice::from_ref(&send), json!({}))
        .await
        .unwrap();
    make_guest_staff();

    let staged = call(&t, app, "send", &staging_host(&t, app), json!({})).await;
    let got = data(&staged);
    assert_eq!(got["messageId"], "m-1");
    assert_eq!(got["deliveredTo"], json!([LOCAL_GUEST_EMAIL]));
    assert_eq!(got["environment"], "staging");
    assert_eq!(
        got["ignoredRecipients"],
        json!({ "to": ["cfo@customer.example"], "cc": ["controller@customer.example"], "bcc": ["audit@customer.example"] })
    );

    let sends = ses_sends(&ses).await;
    assert_eq!(sends.len(), 1);
    assert_eq!(
        sends[0].0,
        vec![LOCAL_GUEST_EMAIL.to_string()],
        "SES delivers to the invoker only"
    );
    assert!(
        sends[0].1.contains("Subject: [staging] JE posted"),
        "{}",
        sends[0].1
    );
    assert!(
        !sends[0].1.contains("customer.example"),
        "no customer address in the message, Reply-To included"
    );
    // The guest's address is `<local-user@example.com>`; the header carries
    // the bare mailbox.
    let invoker = LOCAL_GUEST_EMAIL.trim_matches(['<', '>']);
    assert!(
        sends[0].1.contains(&format!("Reply-To: {invoker}")),
        "replies go to the invoker: {}",
        sends[0].1
    );

    call(&t, app, "send", &production_host(&t, app), json!({})).await;
    let sends = ses_sends(&ses).await;
    assert_eq!(
        sends[1].0,
        vec![
            "audit@customer.example".to_string(),
            "cfo@customer.example".to_string(),
            "controller@customer.example".to_string()
        ],
        "production is unchanged"
    );
}
