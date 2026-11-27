//! The platform checker (an `app_checker` service token: `Action::AppCheck`
//! on staging, `Action::AppCheckProduction` on production; decisions 77 and
//! 78) over the real doors, with the token in `x-api-key` as the promote
//! checks' runner sends it:
//!
//! - the staging page of an app opens to it, and production's, each serving
//!   its own build;
//! - anything but a read on the serve tree is refused before it runs (a `/fn`
//!   call, a form post), on either environment;
//! - the app's own data-plane read passes the read gate on staging and on
//!   production, and the same request is refused by every other data-plane
//!   gate and for another workspace;
//! - it drives a build's checks through the runner routes of the served
//!   `/api` tree, each answering the contract's shape, and that tree refuses
//!   it everything else: the console's reads and writes, the checks' own
//!   `show` and `rerun`, another app;
//! - each environment is checked by its own action, a sandbox by none, and
//!   the checker holds no non-production reach;
//! - a sandbox agent token is refused the runner routes at every layer.
//!
//! Production in depth (its other surfaces, a production attempt, files, and
//! that a visit is not usage): `promote_checks_production`.

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Router, middleware};
use oxy_api_custom_apps::custom_apps_build_checks::handlers::{self, ShowQuery};
use oxy_api_custom_apps::custom_apps_build_checks::requester::Requester;
use oxy_api_custom_apps::custom_apps_build_checks::runner::handlers as runner;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::RequestActor;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::middleware::{AuthState, auth_middleware};
use oxy_auth::token::service::{self, ServiceToken};
use oxy_auth::token::{SandboxAgent, hash_token};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::EntityTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::app_environments_phase_1b::{APP, two_environments};
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    BUILD_ID, ORG_SLUG, Tenant, seeded_tenant, serve_router,
};
use crate::sandbox_agent_token::fixture as agent_token;

/// The checker's token: the prefix and 48 alphanumerics, the shape the
/// infrastructure generates.
pub(crate) const TOKEN: &str = "oxysvc_AppCheckerTestTokenOnly4PromoteChecks0123456789";

/// The `app-checks` service at `app_checker`, installed for this process,
/// with its `users` row created as boot creates it.
pub(crate) async fn with_checker(t: &Tenant) -> &'static ServiceToken {
    let sha256: String = hash_token(TOKEN)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let checker = ServiceToken::new("app-checks", "app_checker", &sha256).expect("an entry");
    service::install(vec![checker]).expect("install the service");
    // Credentials decide the request, as on every deployment: a zero-config
    // test process would run every request as its guest instead.
    oxy_auth::built_in::set_auth_configured(true);
    service::ensure_principals(&t.db)
        .await
        .expect("create the principal");
    service::authenticate(TOKEN).expect("the token names the checker")
}

/// `method` on the app's path URL (`rest` under its base), as the checker.
async fn serve(method: &str, rest: &str, staging: bool) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/customer-apps/{ORG_SLUG}/{APP}/{rest}"))
        .header("accept", "text/html")
        .header("x-api-key", TOKEN);
    if staging {
        request = request.header("x-oxy-app-env", "staging");
    }
    let body = if method == "GET" {
        Body::empty()
    } else {
        Body::from("{}")
    };
    let response = serve_router()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn the_checker_opens_an_apps_two_pages_and_nothing_else_on_the_serve_tree() {
    let t = seeded_tenant().await;
    let (app_id, production_build, staging_build) = two_environments(&t).await;
    with_checker(&t).await;

    let (status, html) = serve("GET", "", true).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&format!("\"buildId\":\"{staging_build}\""))
            && html.contains("\"environment\":\"staging\""),
        "staging's build is served: {html}"
    );

    // Production's page opens to it by its own action, and serves
    // production's build, never the one being checked on staging.
    let (status, html) = serve("GET", "", false).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(&format!("\"buildId\":\"{production_build}\""))
            && html.contains("\"environment\":\"production\""),
        "production's build is served: {html}"
    );

    for staging in [true, false] {
        for (method, rest) in [("POST", "fn/stamp"), ("POST", ""), ("PUT", "x")] {
            let (status, _) = serve(method, rest, staging).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {rest}, staging: {staging}"
            );
        }
        // A function answers a POST only, so a read of its path runs nothing.
        let (status, _) = serve("GET", "fn/stamp", staging).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "staging: {staging}");
    }
    assert!(
        crate::custom_app_functions_fixture::invocations(&t.db, app_id, "stamp")
            .await
            .is_empty(),
        "no function ran for the checker"
    );
}

fn data_headers(app_id: Uuid, staging: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_static(TOKEN));
    headers.insert("x-oxy-app", app_id.to_string().parse().unwrap());
    if staging {
        headers.insert("x-oxy-app-env", HeaderValue::from_static("staging"));
    }
    headers
}

#[tokio::test]
async fn the_checker_reads_its_apps_data_through_the_read_gate_only() {
    use oxy_custom_apps::gate::{check_custom_app_gates, check_custom_app_read_gates};
    let t = seeded_tenant().await;
    let (app_id, _, _) = two_environments(&t).await;
    with_checker(&t).await;
    let workspace = demo_workspace_id();

    let read = check_custom_app_read_gates(&data_headers(app_id, true), workspace).await;
    let ctx = read.unwrap_or_else(|e| panic!("the app's own staging read: {}", e.message()));
    assert_eq!(ctx.project_id, workspace);
    assert!(ctx.as_checker, "admitted as the checker, not as a reader");

    // Every other data-plane door (an ask, an automation run, the metric tree)
    // takes the ordinary gate, which never admits the checker.
    let other_door = check_custom_app_gates(&data_headers(app_id, true), workspace).await;
    assert_eq!(other_door.err().map(|e| e.status()), Some(403));
    // Production's read passes the same gate by its own action, and the
    // ordinary gate refuses the checker there as it does on staging.
    let production = check_custom_app_read_gates(&data_headers(app_id, false), workspace).await;
    let ctx =
        production.unwrap_or_else(|e| panic!("the app's own production read: {}", e.message()));
    assert_eq!(ctx.project_id, workspace);
    let other_door = check_custom_app_gates(&data_headers(app_id, false), workspace).await;
    assert_eq!(other_door.err().map(|e| e.status()), Some(403));
    // The app was not published from another workspace.
    let elsewhere = check_custom_app_read_gates(&data_headers(app_id, true), Uuid::new_v4()).await;
    assert!(elsewhere.is_err(), "another workspace's data");
}

/// The shell's bootstrap answers the checker on the app it checks, on staging and
/// on production, so its pictures carry the app's chrome; with no viewer in it,
/// since no person stands behind the request. Before this, the read answered
/// `401` on every screen of every app with the SDK's shell.
#[tokio::test]
async fn the_checker_reads_the_shells_bootstrap_with_no_viewer_in_it() {
    use oxy_api_custom_apps::custom_apps_shell_context::{ShellContextQuery, get_shell_context};
    let t = seeded_tenant().await;
    let (app_id, _, _) = two_environments(&t).await;
    with_checker(&t).await;
    let workspace = demo_workspace_id();
    let ask = |headers: HeaderMap| async move {
        let query = ShellContextQuery {
            app: Some(app_id.to_string()),
        };
        let response = get_shell_context(Path(workspace), Query(query), None, headers).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body");
        (
            status,
            serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
        )
    };

    // The workspace publishes other apps too, so "alone" below is a filter.
    use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};
    let published = entity::apps::Entity::find()
        .filter(entity::apps::Column::ProjectId.eq(workspace))
        .filter(entity::apps::Column::PublishedAt.is_not_null())
        .count(&t.db)
        .await
        .expect("count the workspace's apps");
    assert!(
        published > 1,
        "only {published} published app(s) in the workspace"
    );

    for staging in [true, false] {
        let (status, body) = ask(data_headers(app_id, staging)).await;
        assert_eq!(status, StatusCode::OK, "staging={staging}: {body}");
        assert!(body["user"].is_null(), "no viewer: {body}");
        assert!(
            body.get("mode").is_none(),
            "the bundle keeps its own mode: {body}"
        );
        let apps = body["apps"].as_array().cloned().unwrap_or_default();
        let ids: Vec<&Value> = apps.iter().map(|a| &a["id"]).collect();
        assert_eq!(
            ids,
            [&json!(app_id)],
            "the rail lists the app checked, alone: {body}"
        );
        assert!(
            body["logo_url"].is_null(),
            "no logo, which would refuse it: {body}"
        );
    }

    // Naming no app, the checker is no reader of this workspace.
    let mut no_app = data_headers(app_id, true);
    no_app.remove("x-oxy-app");
    let (status, _) = ask(no_app).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A Global Admin's service token, for the lockdown case: staff standing that
/// holds `check_apps` as every Global Admin does.
const GLOBAL_ADMIN_TOKEN: &str = "oxysvc_GlobalAdminTestTokenOnlyForTheLockdownCase012345";

/// A fresh org, workspace and app of `t`'s database: nothing else reads them,
/// so a lockdown on the workspace cannot reach another test.
async fn lone_app(t: &Tenant) -> (Uuid, Uuid) {
    use entity::{apps, organizations, workspaces};
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org),
        name: Set("Locked".into()),
        slug: Set(format!("locked-{}", org.simple())),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed org");
    let workspace = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace),
        name: Set("locked".into()),
        org_id: Set(Some(org)),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");
    let app = Uuid::new_v4();
    apps::ActiveModel {
        id: Set(app),
        org_id: Set(org),
        project_id: Set(workspace),
        slug: Set(format!("locked-{}", &app.simple().to_string()[..8])),
        name: Set("Locked".into()),
        branch: Set("main".into()),
        source_repo: Set("acme/locked".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        visibility: Set("org".into()),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed app");
    (workspace, app)
}

/// An org that has locked Oxy out of a workspace has locked out the checker's
/// way in too: the checker and every staff standing that holds `check_apps` (a
/// Global Admin) are refused there. Before, the read gate fell through to the
/// checker's path for any caller the ordinary rule refused, and that path never
/// read the lockdown, so a Global Admin read a locked workspace's warehouse.
#[tokio::test]
async fn a_workspace_locked_against_oxy_is_locked_against_the_checkers_way_in() {
    use oxy_custom_apps::gate::check_custom_app_read_gates;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    let t = seeded_tenant().await;
    let hex =
        |raw: &str| -> String { hash_token(raw).iter().map(|b| format!("{b:02x}")).collect() };
    service::install(vec![
        ServiceToken::new("app-checks", "app_checker", &hex(TOKEN)).expect("the checker"),
        ServiceToken::new("ops", "global_admin", &hex(GLOBAL_ADMIN_TOKEN)).expect("an admin"),
    ])
    .expect("install the services");
    oxy_auth::built_in::set_auth_configured(true);
    service::ensure_principals(&t.db)
        .await
        .expect("create the principals");
    let (workspace, app) = lone_app(&t).await;
    let read = |token: &'static str| {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static(token));
        headers.insert("x-oxy-app", app.to_string().parse().unwrap());
        async move { check_custom_app_read_gates(&headers, workspace).await }
    };

    // Unlocked: the checker by its own way in, the admin as staff.
    let checker = read(TOKEN)
        .await
        .unwrap_or_else(|e| panic!("checker: {}", e.message()));
    assert!(checker.as_checker);
    let admin = read(GLOBAL_ADMIN_TOKEN)
        .await
        .unwrap_or_else(|e| panic!("admin: {}", e.message()));
    assert!(!admin.as_checker);

    entity::workspace_oxy_lockdown::ActiveModel {
        id: Set(Uuid::new_v4()),
        workspace_id: Set(workspace),
        locked_by: Set(None),
        created_at: Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(&t.db)
    .await
    .expect("lock Oxy out");

    for (who, token) in [("checker", TOKEN), ("admin", GLOBAL_ADMIN_TOKEN)] {
        let refused = read(token).await.err().map(|e| e.status());
        assert_eq!(refused, Some(403), "{who} after the lockdown");
    }
}

/// One request to the served `/api` tree as the checker.
async fn api(method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    agent_token::send(method, uri, &[("x-api-key", TOKEN)], body).await
}

#[tokio::test]
async fn the_checker_drives_a_builds_checks_through_the_served_api_and_nothing_else_there() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com/api") };
    let t = seeded_tenant().await;
    let (app_id, production_build, staging_build) = two_environments(&t).await;
    with_checker(&t).await;
    let checks = format!("/customer-apps/{app_id}/builds/{staging_build}/checks");

    let runner = json!({ "label": "GitHub Actions run #1482",
        "url": "https://github.com/oxy-hq/oxygen-internal/actions/runs/123" });
    let claim = json!({ "attempt": 1, "runner": runner });
    let (status, claimed) = api("POST", &format!("{checks}/claim"), Some(claim.clone())).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(
        claimed,
        json!({
            "app": { "id": app_id, "workspace_id": demo_workspace_id(),
                "org_slug": ORG_SLUG, "slug": APP },
            "build": { "id": staging_build, "build_id": "env-1b-staging", "sdk_version": null },
            "environment": "staging",
            "attempt": 1,
            "serving": true,
            "url": format!("https://app-dev.oxygen-hq.com/customer-apps/{ORG_SLUG}/{APP}/"),
            "identity": "app_checker",
            "sdk_floor": "2.30.0",
            "open": ["loads", "design_system"],
            // The build declares no screens; production serves the build
            // before it; a staging attempt has no baseline.
            "screens": null,
            "production": { "id": production_build, "build_id": BUILD_ID },
            "baseline": null,
            "limits": { "screens": 30, "file_bytes": 2_000_000, "files": 200 },
        })
    );

    let (status, settled) = api("POST", &format!("{checks}/functions/settle"), None).await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    assert_eq!(
        settled,
        json!({ "done": true, "status": "skipped",
            "summary": "the build declares no checks (no function has \"check\": true)" })
    );

    let (status, sheets) = api("GET", &format!("{checks}/stylesheets"), None).await;
    assert_eq!(status, StatusCode::OK, "{sheets}");
    let sources = sheets["sources"].as_array().expect("sources");
    assert_eq!(sources.len(), 1, "{sheets}");
    assert_eq!(sources[0]["file"], "index.html");
    assert!(
        sources[0]["css"]
            .as_str()
            .unwrap()
            .contains("<title>fn e2e</title>")
    );

    let report = json!({ "attempt": 1, "checks": [
        { "kind": "loads", "status": "passed", "summary": "ready on desk and phone",
          "detail": { "findings": [], "identity": "app_checker" } },
        { "kind": "design_system", "status": "skipped", "summary": "SDK version unknown",
          "detail": { "findings": [], "sdk_version": null, "sdk_floor": "2.30.0" } },
    ] });
    let (status, reported) = api("POST", &format!("{checks}/report"), Some(report)).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    assert_eq!(reported, json!({ "state": "passed", "open": [] }));

    // Every check has its verdict: a refusal is `{code, message}` at the top.
    let (status, closed) = api("POST", &format!("{checks}/claim"), Some(claim)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{closed}");
    assert_eq!(closed["code"], "attempt_closed");
    assert_eq!(
        closed["message"],
        "every check of attempt 1 already has a verdict"
    );
    let bad = json!({ "checks": [{ "kind": "functions", "status": "passed",
        "detail": { "findings": [] } }] });
    let (status, refused) = api("POST", &format!("{checks}/report"), Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "kind_not_reportable");

    // The rest of the tree is closed to it: the console's layers refuse a
    // caller that holds no console standing, the checks' own routes included.
    for (method, uri) in [
        ("GET", checks.clone()),
        ("POST", format!("{checks}/rerun")),
        ("GET", "/customer-apps".to_string()),
        ("GET", format!("/customer-apps/{app_id}")),
        ("GET", format!("/customer-apps/{app_id}/builds")),
        ("GET", format!("/customer-apps/{app_id}/secrets")),
        (
            "POST",
            format!("/customer-apps/{app_id}/functions/stamp/runs?environment=staging"),
        ),
        (
            "POST",
            format!("/customer-apps/{app_id}/environments/production/promote"),
        ),
        ("DELETE", format!("/customer-apps/{app_id}")),
    ] {
        let (status, body) = api(method, &uri, Some(json!({}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
    }
    // An app that does not exist, and a build that is another app's.
    let nowhere = format!(
        "/customer-apps/{}/builds/{staging_build}/checks/claim",
        Uuid::new_v4()
    );
    let claim = json!({ "runner": { "label": "run" } });
    let (status, _) = api("POST", &nowhere, Some(claim.clone())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let no_build = format!(
        "/customer-apps/{app_id}/builds/{}/checks/stylesheets",
        Uuid::new_v4()
    );
    assert_eq!(api("GET", &no_build, None).await.0, StatusCode::NOT_FOUND);
    // No credential at all is not a caller.
    let (status, _) = agent_token::send("POST", &format!("{checks}/claim"), &[], Some(claim)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The checker as a request's actor: what the auth layer hands a handler.
fn checker_actor(checker: &ServiceToken) -> RequestActor {
    let credential = checker.credential();
    let mut actor = RequestActor::session(AuthenticatedUser {
        id: checker.user_id(),
        email: None,
        name: checker.principal(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: Some(credential.clone()),
    });
    actor.credential = Some(credential);
    actor
}

#[tokio::test]
async fn the_checker_checks_each_environment_by_its_own_action_and_holds_no_reach() {
    use oxy_custom_apps::app_check::{may_check, may_check_staging};
    let t = seeded_tenant().await;
    let (app_id, _, staging_build) = two_environments(&t).await;
    let actor = checker_actor(with_checker(&t).await);
    let app = entity::apps::Entity::find_by_id(app_id)
        .one(&t.db)
        .await
        .unwrap()
        .expect("the app");
    let caller = oxy_server_authz::Caller::from_user(&actor.user);

    assert!(may_check_staging(&t.db, &caller, &app, &AppEnvironment::Staging).await);
    let sandbox = AppEnvironment::parse("dev-a1").expect("a sandbox name");
    // A door only staging's check passes (the staging pin) stays staging's.
    for elsewhere in [&AppEnvironment::Production, &sandbox] {
        assert!(
            !may_check_staging(&t.db, &caller, &app, elsewhere).await,
            "{elsewhere}"
        );
    }
    // Production is checked by its own action, and a sandbox by none.
    assert!(may_check(&t.db, &caller, &app, &AppEnvironment::Staging).await);
    assert!(may_check(&t.db, &caller, &app, &AppEnvironment::Production).await);
    assert!(!may_check(&t.db, &caller, &app, &sandbox).await);
    // No non-production reach: every staff tool that asks for it refuses the
    // checker, the checks' own console routes among them, with no layer in
    // front.
    assert!(!oxy_custom_apps::non_production::may_open_non_production(&t.db, &caller, &app).await);
    let path = || Path((app_id, staging_build.to_string()));
    let shown = handlers::show(
        RefuseSandboxAgent,
        actor.clone(),
        None,
        path(),
        Query(ShowQuery::default()),
    )
    .await;
    assert_eq!(shown.unwrap_err().into_response().status(), 404);
    let rerun = handlers::rerun(RefuseSandboxAgent, actor.clone(), None, path()).await;
    assert_eq!(rerun.unwrap_err().into_response().status(), 404);
    // The same actor, on the runner's door, is admitted.
    let sheets = runner::stylesheets(RefuseSandboxAgent, actor, None, path()).await;
    let Json(sheets) = sheets.unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(sheets.sources[0].file, "index.html");
}

/// `GET …/checks/stylesheets` with only authentication in front of the
/// handler: its `RefuseSandboxAgent` extractor answers the token `403
/// sandbox_token_refused`.
async fn assert_the_extractor_refuses_the_token(checks: &str, bearer: &str) {
    let unfenced = Router::new()
        .route(
            "/customer-apps/{id}/builds/{build}/checks/stylesheets",
            get(runner::stylesheets),
        )
        .layer(middleware::from_fn_with_state(
            AuthState::built_in(SandboxAgent::Admit, oxy::database::client::auth_platform()),
            auth_middleware,
        ));
    let request = Request::get(format!("{checks}/stylesheets"))
        .header("authorization", bearer)
        .body(Body::empty())
        .expect("request");
    let response = unfenced.oneshot(request).await.expect("oneshot");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "sandbox_token_refused", "{body}");
}

#[tokio::test]
async fn a_sandbox_agent_token_is_refused_the_runner_routes_at_every_layer() {
    let agent = agent_token::agent().await;
    let app = &agent.app;
    let build = entity::app_builds::Entity::find()
        .all(&agent.t.db)
        .await
        .unwrap()
        .into_iter()
        .find(|b| b.app_id == app.id && b.build_id == "agent-stg")
        .expect("the staging build");
    let checks = format!("/customer-apps/{}/builds/{}/checks", app.id, build.id);
    let bearer = agent.bearer();

    // The served tree: the token's route allow-list names none of the four.
    for (method, rest) in [
        ("POST", "claim"),
        ("POST", "functions/settle"),
        ("GET", "stylesheets"),
        ("POST", "report"),
    ] {
        let uri = format!("{checks}/{rest}");
        let headers = [("authorization", bearer.as_str())];
        let (status, body) = agent_token::send(method, &uri, &headers, Some(json!({}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {rest}: {body}");
    }
    // With only authentication in front, the handler's own extractor refuses
    // it (the three writes are cases of `second_refusal_writes`).
    assert_the_extractor_refuses_the_token(&checks, &bearer).await;
    // With neither, the model refuses it: the token is no checker and holds
    // no reach that names no environment.
    let actor = agent_token::token_actor(&agent.t, agent.token_id, app);
    let path = || Path((app.id, build.id.to_string()));
    let body = || Bytes::from(json!({ "runner": { "label": "agent" } }).to_string());
    let who = Requester {
        actor: actor.clone(),
        marker: None,
    };
    let claimed = runner::claim(RefuseSandboxAgent, who, path(), HeaderMap::new(), body()).await;
    assert_eq!(claimed.unwrap_err().into_response().status(), 404);
    let sheets = runner::stylesheets(RefuseSandboxAgent, actor, None, path()).await;
    assert_eq!(sheets.unwrap_err().into_response().status(), 404);
}
