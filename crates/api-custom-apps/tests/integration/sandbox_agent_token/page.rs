//! A sandbox agent token loads its own sandbox's **page**: the document, its
//! assets, the reserved `__oxy/*` names, and the page's beacon — on the
//! product host's path URL, with the token as the bearer and
//! `X-Oxy-App-Env` naming the environment.
//!
//! Only that. Production (named, or meant by naming nothing), staging without
//! the grant, a colleague's sandbox, another token's, an unknown one, another
//! app, a tenant host, any other POST: each is `404` in the token's one body,
//! as `/fn` answers it. And a page view is not sandbox activity.
//!
//! Sent through the serve route production mounts, with a token minted
//! through the real route.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::fixture::{Agent, agent, mint, publish_as, published, send};
use super::staging_draft::staging_agent;
use crate::app_environments::{seed_sandbox, serving};
use crate::custom_app_functions_fixture::serve_router;
use crate::sandbox_publish::{build_pk, publish_to_sandbox};

const OWN: &str = "dev-own";
const COLLEAGUES: &str = "dev-col";
const OTHERS: &str = "dev-other";
const SIBLING: &str = "sbx-sibling";

/// What the serve route answered.
struct Page {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Page {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// One request to `/customer-apps/<org>/<slug>/<rest>`, as the token sends it.
async fn page(
    agent: &Agent,
    method: &str,
    slug: &str,
    rest: &str,
    headers: &[(&str, &str)],
    body: &'static str,
) -> Page {
    let uri = format!("/customer-apps/{}/{slug}/{rest}", agent.t.org_slug);
    let bearer = agent.bearer();
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", bearer.as_str());
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if !body.is_empty() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(Body::from(body)).expect("request");
    let response = serve_router().oneshot(request).await.expect("oneshot");
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("read the body");
    Page {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// `GET <rest>` of the token's own app, in `environment` when given.
async fn get(agent: &Agent, environment: Option<&str>, rest: &str) -> Page {
    let slug = agent.app.slug.clone();
    match environment {
        Some(env) => page(agent, "GET", &slug, rest, &[("x-oxy-app-env", env)], "").await,
        None => page(agent, "GET", &slug, rest, &[], "").await,
    }
}

const BATCH: &str = r#"{"v":1,"events":[{"n":"oxy-pageview","p":{"path":"/"}}]}"#;

/// The token's body for a refusal: `404 not_found`.
fn assert_refused(what: &str, answer: &Page) {
    assert_eq!(
        answer.status,
        StatusCode::NOT_FOUND,
        "{what}: {}",
        answer.body
    );
    assert_eq!(
        answer.json()["code"],
        "not_found",
        "{what}: the token's one body, got {}",
        answer.body
    );
}

/// The token with a sandbox of its own on a build, beside a colleague's
/// sandbox and another token's (each on a build of its own), and a second app
/// of the same workspace.
struct Scene {
    agent: Agent,
    own_build: Uuid,
    colleague_build: Uuid,
    other_build: Uuid,
}

async fn scene_of(agent: Agent) -> Scene {
    published(&agent.t, SIBLING).await;
    let (status, created) = agent.create(OWN).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let fields = [("build_id", "own-1"), ("environment", OWN)];
    let (status, body) = agent.publish("own", &fields).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    seed_sandbox(&agent.t.db, agent.app.id, COLLEAGUES, agent.t.guest_id).await;
    publish_to_sandbox(&agent.t, &agent.app.slug, "col-1", "col")
        .await
        .expect("a person's publish to their sandbox");

    let (_, other_secret) = mint(&agent.cookie, &[agent.app.id]).await;
    let other = format!("Bearer {other_secret}");
    let uri = format!("/customer-apps/{}/environments", agent.app.id);
    let named = Some(json!({ "name": OTHERS }));
    let (status, body) = send("POST", &uri, &[("authorization", &other)], named).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let fields = [("build_id", "other-1"), ("environment", OTHERS)];
    let (status, body) = publish_as(&other, &agent.t, &agent.app.slug, "other", &fields).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let db = &agent.t.db;
    let build = |id: &'static str| async move {
        build_pk(db, agent.app.id, id).await.expect("a build row")
    };
    let (own_build, colleague_build, other_build) = (
        build("own-1").await,
        build("col-1").await,
        build("other-1").await,
    );
    Scene {
        agent,
        own_build,
        colleague_build,
        other_build,
    }
}

async fn scene() -> Scene {
    scene_of(agent().await).await
}

#[tokio::test]
async fn a_token_loads_the_page_of_its_own_sandbox_and_it_is_that_sandboxes_build() {
    let s = scene().await;
    let a = &s.agent;

    // The document at the app's root, with and without the slash, and by name.
    for rest in ["", "index.html"] {
        let document = get(a, Some(OWN), rest).await;
        assert_eq!(
            document.status,
            StatusCode::OK,
            "/{rest}: {}",
            document.body
        );
        assert!(
            document.body.contains("<title>sandbox</title>"),
            "{}",
            document.body
        );
    }
    let head = page(a, "HEAD", &a.app.slug, "", &[("x-oxy-app-env", OWN)], "").await;
    assert_eq!(head.status, StatusCode::OK, "HEAD");

    // It is THIS sandbox's build: the build the sandbox serves, and neither
    // production's, staging's, nor another sandbox's.
    let live = get(a, Some(OWN), "__oxy/build").await;
    assert_eq!(live.status, StatusCode::OK, "{}", live.body);
    assert_eq!(live.json()["buildId"], json!(s.own_build));
    let pointers = serving(&a.t.db, a.app.id).await;
    for other in [
        pointers.production.expect("production's build"),
        pointers.staging.expect("staging's build"),
        s.colleague_build,
        s.other_build,
    ] {
        assert_ne!(s.own_build, other);
    }

    // The reserved names answer for the sandbox, as for any viewer.
    for rest in [
        "__oxy/sw.js",
        "__oxy/manifest.webmanifest",
        "__oxy/icon.svg",
    ] {
        let reserved = get(a, Some(OWN), rest).await;
        assert_eq!(reserved.status, StatusCode::OK, "{rest}: {}", reserved.body);
    }
    // A path the bundle does not have is the bundle's own answer (an SPA
    // shell), not the fence's `not_found`: the request was admitted.
    let missing = get(a, Some(OWN), "assets/nothing.js").await;
    assert_ne!(missing.json()["code"], "not_found", "{}", missing.body);
}

#[tokio::test]
async fn every_other_environment_is_refused_in_the_tokens_one_body() {
    let s = scene().await;
    let a = &s.agent;
    let pointers = serving(&a.t.db, a.app.id).await;
    assert!(pointers.production.is_some() && pointers.staging.is_some());

    // Production, named or meant by naming nothing; staging without the
    // grant; a sandbox a person made; another token's; one that does not exist.
    let refused: [(&str, Option<&str>); 6] = [
        ("no header (production)", None),
        ("production", Some("production")),
        ("staging, not granted", Some("staging")),
        ("a colleague's sandbox", Some(COLLEAGUES)),
        ("another token's sandbox", Some(OTHERS)),
        ("a sandbox with no row", Some("dev-nobody")),
    ];
    for (what, environment) in refused {
        for rest in ["", "index.html", "__oxy/build", "__oxy/sw.js"] {
            let answer = get(a, environment, rest).await;
            assert_refused(&format!("{what} /{rest}"), &answer);
            assert!(
                !answer.body.contains(&s.colleague_build.to_string())
                    && !answer.body.contains(&s.other_build.to_string()),
                "{what}: leaked a build id: {}",
                answer.body
            );
        }
    }

    // Another app of the same workspace, in a sandbox of the same name.
    let sibling = page(a, "GET", SIBLING, "", &[("x-oxy-app-env", OWN)], "").await;
    assert_refused("another app", &sibling);

    // A subdomain host decides the environment before the header does.
    let host = format!(
        "{OWN}--{}--{}.customer-apps.oxygen-hq.com",
        a.t.org_slug, a.app.slug
    );
    for headers in [
        vec![("host", host.as_str())],
        vec![("host", host.as_str()), ("x-oxy-app-env", OWN)],
    ] {
        let slug = a.app.slug.clone();
        let answer = page(a, "GET", &slug, "", &headers, "").await;
        assert_refused(&format!("{headers:?}"), &answer);
    }
}

#[tokio::test]
async fn a_token_granted_staging_loads_staging_and_still_not_production() {
    let s = scene_of(staging_agent().await).await;
    let a = &s.agent;
    let pointers = serving(&a.t.db, a.app.id).await;

    let live = get(a, Some("staging"), "__oxy/build").await;
    assert_eq!(live.status, StatusCode::OK, "{}", live.body);
    assert_eq!(
        live.json()["buildId"],
        json!(pointers.staging.expect("staging"))
    );
    assert_eq!(get(a, Some("staging"), "").await.status, StatusCode::OK);

    // Its own sandbox, as before.
    let own = get(a, Some(OWN), "__oxy/build").await;
    assert_eq!(own.json()["buildId"], json!(s.own_build));

    for environment in [None, Some("production"), Some(COLLEAGUES), Some(OTHERS)] {
        assert_refused(&format!("{environment:?}"), &get(a, environment, "").await);
    }
}

#[tokio::test]
async fn the_beacon_is_accepted_in_its_own_sandbox_and_no_other_post_is() {
    let s = scene().await;
    let a = &s.agent;
    let slug = a.app.slug.clone();
    let own = [("x-oxy-app-env", OWN)];

    let beacon = page(a, "POST", &slug, "__oxy/beacon", &own, BATCH).await;
    assert_eq!(beacon.status, StatusCode::NO_CONTENT, "{}", beacon.body);

    // Every other POST shape is the fence's 404.
    for rest in [
        "something-else",
        "",
        "index.html",
        "__oxy/build",
        "__oxy/beacon/extra",
        "fn/",
    ] {
        let answer = page(a, "POST", &slug, rest, &own, BATCH).await;
        assert_refused(&format!("POST /{rest}"), &answer);
    }
    for method in ["PUT", "DELETE", "PATCH"] {
        let answer = page(a, method, &slug, "index.html", &own, "").await;
        assert_refused(method, &answer);
    }
    // A function is still POST only, and `GET fn/<name>` is not a page.
    let answer = page(a, "GET", &slug, "fn/whoami", &own, "").await;
    assert_refused("GET fn/whoami", &answer);

    // And the beacon is the sandbox's own: nowhere else.
    for environment in [
        None,
        Some("production"),
        Some("staging"),
        Some(COLLEAGUES),
        Some(OTHERS),
    ] {
        let headers: Vec<(&str, &str)> = environment
            .map(|e| ("x-oxy-app-env", e))
            .into_iter()
            .collect();
        let answer = page(a, "POST", &slug, "__oxy/beacon", &headers, BATCH).await;
        assert_refused(&format!("beacon in {environment:?}"), &answer);
    }
}

/// What a sandbox's idle clock reads: its pointer's `updated_at` and its
/// function invocations (`custom_apps_sandboxes::activity`).
async fn activity(db: &DatabaseConnection, app: Uuid) -> (String, i64) {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT updated_at::text AS at, \
                    (SELECT count(*) FROM app_function_invocations i \
                      WHERE i.app_id = e.app_id AND i.environment = e.name) AS n \
               FROM app_environments e WHERE e.app_id = $1 AND e.name = $2",
            [app.into(), OWN.into()],
        ))
        .await
        .expect("read the sandbox")
        .expect("the sandbox row");
    (
        row.try_get("", "at").expect("updated_at"),
        row.try_get("", "n").expect("n"),
    )
}

#[tokio::test]
async fn loading_the_page_is_not_sandbox_activity() {
    let s = scene().await;
    let a = &s.agent;
    let before = activity(&a.t.db, a.app.id).await;

    for rest in ["", "index.html", "__oxy/build", "__oxy/sw.js"] {
        assert_eq!(
            get(a, Some(OWN), rest).await.status,
            StatusCode::OK,
            "/{rest}"
        );
    }
    let slug = a.app.slug.clone();
    let own = [("x-oxy-app-env", OWN)];
    let beacon = page(a, "POST", &slug, "__oxy/beacon", &own, BATCH).await;
    assert_eq!(beacon.status, StatusCode::NO_CONTENT);

    assert_eq!(
        activity(&a.t.db, a.app.id).await,
        before,
        "no pointer moved and no function ran"
    );
}

#[tokio::test]
async fn an_ended_token_is_401_on_the_page_not_a_sign_in_redirect() {
    let s = scene().await;
    let a = &s.agent;
    assert_eq!(get(a, Some(OWN), "").await.status, StatusCode::OK);

    a.end().await;

    let answer = get(a, Some(OWN), "").await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert!(
        answer.headers.get("location").is_none(),
        "no redirect to a sign-in"
    );
}
