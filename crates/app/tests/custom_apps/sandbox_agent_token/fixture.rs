//! A sandbox agent token, as a handler or an op sees it — and a real one:
//! minted through the token route under the minter's session, and presented
//! to the routers production mounts.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use entity::{app_admins, apps, users};
use oxy_app::server::api::custom_apps_publish::publish;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::{AppSandboxGrant, CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use oxy_authz::{PlatformRole, RoleCeiling, TokenGrant};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection};
use sea_orm::{EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FnCall, FunctionSpec, Tenant, call_function_with};
use crate::custom_app_functions_fixture::{seeded_tenant, serve_router};
use crate::sandbox_publish::{app_row, bundle, input};

pub(crate) const APP: &str = "sbx-agent";
pub(crate) const BOUNDARY: &str = "sbx-agent-boundary";

pub(crate) async fn app_model(db: &DatabaseConnection, id: Uuid) -> apps::Model {
    apps::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("read the app")
        .expect("the app")
}

/// The credential admission builds for token `token_id`, minted by the guest
/// and granted `app`.
pub(crate) fn credential(t: &Tenant, token_id: Uuid, app: &apps::Model) -> CredentialContext {
    CredentialContext {
        token_id,
        kind: StoredKind::SandboxAgent,
        principal_user_id: t.guest_id,
        all_access: false,
        platform: true,
        partner: false,
        name: "agent".into(),
        display_prefix: "oxy_sbx_Ab3x".into(),
        legacy_api_key_id: None,
        grants: vec![TokenGrant {
            org_id: app.org_id,
            workspace_id: Some(app.project_id),
            ceiling: RoleCeiling::Admin,
        }],
        app_publish: Vec::new(),
        app_sandbox: vec![AppSandboxGrant {
            org_id: app.org_id,
            app_id: app.id,
        }],
        blocked_orgs: Vec::new(),
        service_account: None,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(8)),
    }
}

/// The guest, on a sandbox agent token granted `app`, as a request's actor.
pub(crate) fn token_actor(t: &Tenant, token_id: Uuid, app: &apps::Model) -> RequestActor {
    let credential = credential(t, token_id, app);
    let mut actor = RequestActor::session(AuthenticatedUser {
        id: t.guest_id,
        email: Some(LOCAL_GUEST_EMAIL.to_string()),
        name: "Guest".to_string(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: Some(credential.clone()),
    });
    actor.credential = Some(credential);
    actor
}

/// Two functions, told apart across builds by `mark`. `whoami` is the route
/// call: it reports its build, its channel, the caller's app role and a
/// secret, and makes a third-party write when asked. `smoke` is the check:
/// the same write, which the non-production policy holds.
pub(crate) fn functions(mark: &str) -> Vec<FunctionSpec> {
    // `FunctionSpec::js` is `&'static str`; a test leaks two short strings.
    let marked = |js: &str| -> &'static str {
        Box::leak(format!("const MARK = {mark:?};\n{js}").into_boxed_str())
    };
    vec![
        FunctionSpec {
            name: "whoami",
            manifest: json!({ "route": true }),
            js: marked(WHOAMI_JS),
        },
        FunctionSpec {
            name: "smoke",
            manifest: json!({ "check": true }),
            js: marked(SMOKE_JS),
        },
    ]
}

const WHOAMI_JS: &str = r#"
export default async (req, ctx) => {
  const { write } = JSON.parse(req.body || "{}");
  const out = { build: MARK, channel: ctx.channel, role: ctx.user.appRole ?? null, secret: ctx.env.TOKEN ?? null };
  if (write) out.write = (await ctx.fetch("https://api.example.com/orders", { method: "POST", body: "{}" })).status;
  return Response.json(out);
};
"#;

const SMOKE_JS: &str = r#"
export default async (req, ctx) => {
  const held = await ctx.fetch("https://api.example.com/orders", { method: "POST", body: "{}" });
  return Response.json({ build: MARK, channel: ctx.channel, write: held.status });
};
"#;

pub(crate) fn tarball(slug: &str, mark: &str) -> Vec<u8> {
    bundle(
        slug,
        &functions(mark),
        json!({ "env": { "TOKEN": {} } }),
        &[],
    )
}

/// A real sandbox agent token and everything around it: the tenant, the app
/// it is granted, the minter's browser session, and the token's secret.
pub(crate) struct Agent {
    pub t: Tenant,
    pub app: apps::Model,
    pub token_id: Uuid,
    pub secret: String,
    /// The minter's browser session, for what only a person does.
    pub cookie: String,
}

/// `slug` published to production and to staging, as the guest.
pub(crate) async fn published(t: &Tenant, slug: &str) -> apps::Model {
    let mut production = input(t, slug, "agent-prod", tarball(slug, "production"));
    production.promote = true;
    let app_id = publish(production).await.expect("production").app_id;
    publish(input(t, slug, "agent-stg", tarball(slug, "staging")))
        .await
        .expect("staging");
    app_row(&t.db, app_id).await
}

/// Make the guest Oxy staff by a **grant row** — a Global Admin over every
/// org — not by `OXY_OWNER`: a grant row is what can be taken away.
pub(crate) async fn grant_staff(db: &DatabaseConnection, email: &str) {
    app_admins::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(email.to_string()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        role: ActiveValue::Set(PlatformRole::GlobalAdmin.as_str().to_string()),
        scope_all: ActiveValue::Set(true),
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed the staff grant");
}

/// Take the minter's staff grant away. No cache is touched: a sandbox agent
/// token must see this on its next request by itself.
pub(crate) async fn revoke_staff(db: &DatabaseConnection, email: &str) {
    app_admins::Entity::delete_many()
        .filter(app_admins::Column::Email.eq(email))
        .exec(db)
        .await
        .expect("delete the staff grant");
}

async fn guest_row(t: &Tenant) -> users::Model {
    users::Entity::find_by_id(t.guest_id)
        .one(&t.db)
        .await
        .expect("read the guest")
        .expect("the guest")
}

/// A browser session for `user`, as the `Cookie` header carries it.
pub(crate) async fn session_of(user: users::Model) -> String {
    let jwt = oxy_app::server::api::auth::create_auth_token(user)
        .await
        .expect("mint a session");
    format!("oxy_session={jwt}")
}

/// The seeded tenant with its app on a production and a staging build, the
/// guest made staff by a grant row, authentication switched on, and a token
/// minted for the app under the guest's session through the real route.
pub(crate) async fn agent() -> Agent {
    let t = seeded_tenant().await;
    let app = published(&t, APP).await;
    grant_staff(&t.db, LOCAL_GUEST_EMAIL).await;
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com") };
    oxy_auth::built_in::set_auth_configured(true);
    oxy_auth::token::cache::clear();
    let cookie = session_of(guest_row(&t).await).await;
    let (token_id, secret) = mint(&cookie, &[app.id]).await;
    Agent {
        t,
        app,
        token_id,
        secret,
        cookie,
    }
}

/// Mint a sandbox agent token for `apps` under the session `cookie`.
pub(crate) async fn mint(cookie: &str, apps: &[Uuid]) -> (Uuid, String) {
    let body = json!({ "name": "agent", "kind": "sandbox_agent", "apps": apps });
    let (status, minted) = send("POST", "/user/tokens", &[("cookie", cookie)], Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "mint: {minted}");
    let id = minted["token"]["id"].as_str().expect("a token id");
    let secret = minted["secret"].as_str().expect("the secret").to_string();
    (Uuid::parse_str(id).expect("a uuid"), secret)
}

/// The flat `/api` tree as `oxy-app` mounts it: the public routes, then the
/// global ones behind the real authentication, fences and guards.
pub(crate) fn api_router() -> axum::Router {
    oxy_app::server::router::flat_api_surface(axum::Router::new(), Vec::new())
}

async fn answer(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("read the body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

/// One request to the flat `/api` tree. A body that is not JSON comes back
/// as a string.
pub(crate) async fn send(
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let request = request.body(body).expect("request");
    answer(api_router().oneshot(request).await.expect("oneshot")).await
}

impl Agent {
    pub(crate) fn bearer(&self) -> String {
        format!("Bearer {}", self.secret)
    }

    /// One request to `/api` with the token.
    pub(crate) async fn api(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        send(method, uri, &[("authorization", &self.bearer())], body).await
    }

    pub(crate) async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.api("GET", uri, None).await
    }

    /// `/customer-apps/<this app>/<rest>` with the token.
    pub(crate) async fn app_get(&self, rest: &str) -> (StatusCode, Value) {
        self.get(&format!("/customer-apps/{}/{rest}", self.app.id))
            .await
    }

    /// Create the sandbox `name` of the token's app, by route.
    pub(crate) async fn create(&self, name: &str) -> (StatusCode, Value) {
        let uri = format!("/customer-apps/{}/environments", self.app.id);
        self.api("POST", &uri, Some(json!({ "name": name }))).await
    }

    /// `POST /customer-apps/publish` as `oxyc publish` sends it: the bundle
    /// marked `mark`, the given text `fields` beside the app and workspace.
    pub(crate) async fn publish(&self, mark: &str, fields: &[(&str, &str)]) -> (StatusCode, Value) {
        publish_as(&self.bearer(), &self.t, &self.app.slug, mark, fields).await
    }

    /// `POST …/fn/<name>` of the token's app over the serve route, with the
    /// token; `environment` is sent as `X-Oxy-App-Env` when given.
    pub(crate) async fn call(&self, environment: Option<&str>, name: &str, body: Value) -> FnCall {
        self.call_app(&self.app.slug, environment, name, body).await
    }

    pub(crate) async fn call_app(
        &self,
        slug: &str,
        environment: Option<&str>,
        name: &str,
        body: Value,
    ) -> FnCall {
        let bearer = self.bearer();
        let mut headers = vec![("authorization", bearer.as_str())];
        if let Some(environment) = environment {
            headers.push(("x-oxy-app-env", environment));
        }
        call_function_with(&self.t.org_slug, slug, name, body, &headers).await
    }
}

/// The multipart publish, with the credential `authorization` carries.
pub(crate) async fn publish_as(
    authorization: &str,
    t: &Tenant,
    slug: &str,
    mark: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (workspace, org) = (demo_workspace_id().to_string(), t.org_id.to_string());
    let mut all = vec![
        ("app", slug),
        ("project", workspace.as_str()),
        ("org_id", org.as_str()),
    ];
    all.extend_from_slice(fields);
    let request = Request::builder()
        .method("POST")
        .uri("/customer-apps/publish")
        .header("authorization", authorization)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(&all, &tarball(slug, mark))))
        .expect("request");
    answer(api_router().oneshot(request).await.expect("oneshot")).await
}

/// The multipart body `oxyc publish` sends: text `fields`, then the bundle.
pub(crate) fn multipart(fields: &[(&str, &str)], tarball: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        let part = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        );
        body.extend_from_slice(part.as_bytes());
    }
    let file = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"bundle\"; \
         filename=\"bundle.tar.gz\"\r\nContent-Type: application/gzip\r\n\r\n"
    );
    body.extend_from_slice(file.as_bytes());
    body.extend_from_slice(tarball);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

/// One request to the custom-app serve route, with `headers`.
pub(crate) async fn serve(method: &str, uri: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(Body::empty()).expect("request");
    serve_router()
        .oneshot(request)
        .await
        .expect("oneshot")
        .status()
}
