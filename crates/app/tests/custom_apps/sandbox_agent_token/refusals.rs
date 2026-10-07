//! Everything a sandbox agent token is refused (sandbox agent credential
//! design §1 "Never, by construction", §7.2): production and staging through
//! every route, another app — one published from the same workspace included
//! — a colleague's sandbox and another token's, a channel publish and a
//! promote, a secret's value, the staff console, the public reads, a
//! subdomain host, `?api_key=`, and a credential stored as a secret.
//!
//! Each refusal is sent through the routers production mounts, and the test
//! ends by showing nothing moved.

use axum::http::StatusCode;
use entity::{app_environments, app_function_invocations};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, agent, mint, publish_as, published, send, serve};
use crate::app_environments::seed_sandbox;
use crate::sandbox_routes::sandbox_rows;

const OWN: &str = "dev-own";
/// A sandbox a person created.
const COLLEAGUES: &str = "dev-col";
/// A sandbox another token created.
const OTHERS: &str = "dev-other";
const SIBLING: &str = "sbx-sibling";

/// The token with a sandbox of its own on a build, beside a colleague's
/// sandbox, another token's, and a second app of the same workspace.
struct Scene {
    agent: Agent,
    other_secret: String,
    sibling: entity::apps::Model,
    /// A production invocation of the token's app.
    production_invocation: Uuid,
    /// The host of the token's own sandbox.
    sandbox_host: String,
}

async fn scene() -> Scene {
    let agent = agent().await;
    let sibling = published(&agent.t, SIBLING).await;
    let (status, created) = agent.create(OWN).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let sandbox_host = created["url"]
        .as_str()
        .expect("the sandbox's host")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string();
    let fields = [("build_id", "own-1"), ("environment", OWN)];
    let (status, body) = agent.publish("own", &fields).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    seed_sandbox(&agent.t.db, agent.app.id, COLLEAGUES, agent.t.guest_id).await;
    let (_, other_secret) = mint(&agent.cookie, &[agent.app.id]).await;
    let other = format!("Bearer {other_secret}");
    let environments = format!("/customer-apps/{}/environments", agent.app.id);
    let named = Some(json!({ "name": OTHERS }));
    let (status, body) = send("POST", &environments, &[("authorization", &other)], named).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let production_invocation = production_invocation(&agent).await;
    Scene {
        agent,
        other_secret,
        sibling,
        production_invocation,
        sandbox_host,
    }
}

/// An invocation row in production, as a viewer's call leaves one.
async fn production_invocation(agent: &Agent) -> Uuid {
    let id = Uuid::new_v4();
    app_function_invocations::ActiveModel {
        id: ActiveValue::Set(id),
        app_id: ActiveValue::Set(agent.app.id),
        build_id: ActiveValue::Set(agent.app.published_build_id.expect("a promoted build")),
        function_name: ActiveValue::Set("whoami".into()),
        mode: ActiveValue::Set("route".into()),
        user_id: ActiveValue::Set(Some(agent.t.guest_id)),
        status: ActiveValue::Set("success".into()),
        duration_ms: ActiveValue::Set(Some(1)),
        error: ActiveValue::Set(None),
        cancel_requested_at: ActiveValue::Set(None),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        idempotency_key: ActiveValue::Set(None),
        result_body: ActiveValue::Set(None),
        result_status: ActiveValue::Set(None),
        request_hash: ActiveValue::Set(None),
        failure_fingerprint: ActiveValue::Set(None),
        environment: ActiveValue::Set("production".into()),
        credential_token_id: ActiveValue::Set(None),
    }
    .insert(&agent.t.db)
    .await
    .expect("seed a production invocation");
    id
}

/// Every environment that is not the token's own, as a route may name it.
const NOT_OWN: [&str; 4] = ["production", "staging", COLLEAGUES, OTHERS];

/// The console routes of the token's own app: each is `404` for production
/// (named, or meant by naming nothing), staging, the colleague's sandbox and
/// the other token's.
async fn other_environments_of_its_app_do_not_exist(s: &Scene) {
    let app = s.agent.app.id;
    let mut reads = vec![
        "functions".to_string(),
        "functions/whoami/invocations".to_string(),
        "invocations".to_string(),
        "secrets".to_string(),
        format!("invocations/{}/held", s.production_invocation),
        format!("function-runs/{}", Uuid::new_v4()),
    ];
    for environment in NOT_OWN {
        reads.push(format!("environments/{environment}"));
        reads.push(format!("functions?environment={environment}"));
        reads.push(format!(
            "functions/whoami/invocations?environment={environment}"
        ));
        reads.push(format!("invocations?environment={environment}"));
        reads.push(format!("secrets?environment={environment}"));
    }
    for read in reads {
        let (status, body) = s.agent.app_get(&read).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {read}: {body}");
    }

    let mut writes: Vec<(&str, String, Option<Value>)> = vec![
        ("POST", "functions/smoke/runs".into(), None),
        ("DELETE", "secrets/TOKEN".into(), None),
        (
            "POST",
            "secrets".into(),
            Some(json!({ "key": "K", "value": "v" })),
        ),
    ];
    for environment in NOT_OWN {
        let set = json!({ "key": "K", "value": "v", "environment": environment });
        writes.push(("POST", "secrets".into(), Some(set)));
        writes.push((
            "DELETE",
            format!("secrets/TOKEN?environment={environment}"),
            None,
        ));
        writes.push((
            "POST",
            format!("functions/smoke/runs?environment={environment}"),
            None,
        ));
        writes.push(("DELETE", format!("environments/{environment}"), None));
    }
    for (method, rest, body) in writes {
        let uri = format!("/customer-apps/{app}/{rest}");
        let (status, answer) = s.agent.api(method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {rest}: {answer}");
    }
}

/// A publish that would move staging's or production's pointer is the one
/// `403`: `sandbox_token_refused`, as JSON a client can branch on. A publish
/// to a sandbox that is not the token's, or to another app, is a `404`.
async fn a_publish_reaches_only_its_own_sandbox(s: &Scene) {
    let channel: &[(&str, &str)] = &[("build_id", "to-staging")];
    let promote: &[(&str, &str)] = &[("build_id", "to-prod"), ("promote", "true")];
    let both: &[(&str, &str)] = &[
        ("build_id", "both"),
        ("environment", OWN),
        ("promote", "true"),
    ];
    let live: &[(&str, &str)] = &[("build_id", "live"), ("channel", "published")];
    for fields in [channel, promote, both, live] {
        let (status, body) = s.agent.publish("x", fields).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{fields:?}: {body}");
        assert_eq!(body["code"], "sandbox_token_refused", "{body}");
        assert_eq!(body["error"], "sandbox_token_refused", "{body}");
    }
    for environment in [COLLEAGUES, OTHERS, "dev-nobody"] {
        let fields = [("build_id", "elsewhere"), ("environment", environment)];
        let (status, body) = s.agent.publish("x", &fields).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{environment}: {body}");
    }
    let fields = [("build_id", "sibling-1"), ("environment", OWN)];
    let bearer = s.agent.bearer();
    let (status, body) = publish_as(&bearer, &s.agent.t, SIBLING, "x", &fields).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another app: {body}");
}

/// Everything outside the loop on `/api` is `404`: every write to an app's
/// production or staging state (promote, unpublish, rollback, delete, update,
/// create, the batches, storage, publishers, the audience, a publish token),
/// a secret's value even in its own sandbox, another app of the same
/// workspace, the staff console, its minter's own routes.
async fn nothing_else_on_the_api_exists(s: &Scene) {
    let (app, sibling) = (s.agent.app.id, s.sibling.id);
    let outside = [
        ("POST", format!("/customer-apps/{app}/publish")),
        ("DELETE", format!("/customer-apps/{app}/publish")),
        ("POST", format!("/customer-apps/{app}/rollback")),
        ("DELETE", format!("/customer-apps/{app}")),
        ("PATCH", format!("/customer-apps/{app}")),
        ("POST", "/customer-apps".to_string()),
        ("POST", "/customer-apps/batch/publish".to_string()),
        ("POST", "/customer-apps/batch/promote-latest".to_string()),
        ("POST", "/customer-apps/batch/unpublish".to_string()),
        ("POST", "/customer-apps/batch/delete".to_string()),
        ("POST", "/customer-apps/storage/sweep".to_string()),
        ("POST", format!("/customer-apps/{app}/storage/delete")),
        ("POST", format!("/customer-apps/{app}/publishers")),
        (
            "DELETE",
            format!("/customer-apps/{app}/publishers/{sibling}"),
        ),
        ("POST", format!("/admin/apps/{app}/publish")),
        ("DELETE", format!("/admin/apps/{app}/publish")),
        ("DELETE", format!("/admin/apps/{app}")),
        ("PUT", format!("/admin/apps/{app}/access")),
        ("POST", "/admin/app-publish-tokens".to_string()),
        (
            "GET",
            format!("/customer-apps/{app}/secrets/TOKEN/value?environment={OWN}"),
        ),
        ("GET", format!("/customer-apps/{app}/secrets/TOKEN/value")),
        ("GET", format!("/customer-apps/{sibling}/environments")),
        ("POST", format!("/customer-apps/{sibling}/environments")),
        (
            "GET",
            format!("/customer-apps/{sibling}/functions?environment={OWN}"),
        ),
        (
            "GET",
            format!("/customer-apps/{sibling}/secrets?environment={OWN}"),
        ),
        ("GET", "/customer-apps".to_string()),
        (
            "GET",
            format!("/admin/apps/{app}/functions?environment={OWN}"),
        ),
        (
            "GET",
            format!("/admin/apps/{app}/invocations?environment={OWN}"),
        ),
        ("GET", "/admin/sandbox-agent-tokens".to_string()),
        ("GET", "/admin/standing-tokens".to_string()),
        ("GET", "/user/tokens".to_string()),
        ("GET", "/orgs".to_string()),
        ("GET", "/assume".to_string()),
    ];
    for (method, uri) in outside {
        let body = (method == "POST").then(|| json!({ "name": "dev-x" }));
        let (status, answer) = s.agent.api(method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {answer}");
    }
    // `?api_key=`: the token is never honoured from a URL.
    let in_url = format!("/auth/token?api_key={}", s.agent.secret);
    assert_eq!(
        send("GET", &in_url, &[], None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

/// A secret value shaped like an Oxy credential is refused, in the token's
/// own sandbox, and nothing is stored.
async fn a_credential_is_not_stored_as_a_secret(s: &Scene) {
    let uri = format!("/customer-apps/{}/secrets", s.agent.app.id);
    for value in [s.agent.secret.clone(), format!("Bearer {}", s.other_secret)] {
        let set = json!({ "key": "LEAK", "value": value, "environment": OWN });
        let (status, body) = s.agent.api("POST", &uri, Some(set)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body.to_string().contains("credential_shaped_value"),
            "{body}"
        );
    }
    let (status, listed) = s.agent.get(&format!("{uri}?environment={OWN}")).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(!listed.to_string().contains("LEAK"), "{listed}");
}

/// The public reads refuse the token where they authenticate; `/logs` admits
/// it, and then answers only for its own sandbox.
async fn the_public_reads_are_closed(s: &Scene) {
    let app = format!("/customer-apps/{}/{}", s.agent.t.org_slug, s.agent.app.slug);
    for read in ["errors", "debug", "health", "availability"] {
        let (status, body) = s.agent.get(&format!("{app}/{read}")).await;
        assert!(
            matches!(status, StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND),
            "{read}: {status} {body}"
        );
    }
    let mut logs = vec![format!("{app}/logs")];
    logs.extend(NOT_OWN.map(|environment| format!("{app}/logs?environment={environment}")));
    let sibling = format!("/customer-apps/{}/{SIBLING}", s.agent.t.org_slug);
    logs.push(format!("{sibling}/logs?environment={OWN}"));
    for read in logs {
        let (status, body) = s.agent.get(&read).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {read}: {body}");
    }
    let external = oxy_app::server::router::external_auth_layers(axum::Router::new().route(
        "/probe",
        axum::routing::get(|| async { StatusCode::NO_CONTENT }),
    ));
    let request = axum::http::Request::get("/probe")
        .header("authorization", s.agent.bearer())
        .body(axum::body::Body::empty())
        .expect("request");
    let response = tower::ServiceExt::oneshot(external, request)
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "/external/api");
}

/// `/fn` runs for the token only in its own sandbox, named by the header on
/// the product host. No header is production; a subdomain host decides the
/// environment before the header does; a bundle is never the token's to read.
async fn fn_runs_only_in_its_own_sandbox(s: &Scene) {
    let own = s.agent.call(Some(OWN), "whoami", json!({})).await;
    assert_eq!(own.status, StatusCode::OK, "its own sandbox: {}", own.raw);

    for environment in [
        None,
        Some("production"),
        Some("staging"),
        Some(COLLEAGUES),
        Some(OTHERS),
    ] {
        let refused = s.agent.call(environment, "whoami", json!({})).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{environment:?}: {}",
            refused.raw
        );
    }
    let sibling = s
        .agent
        .call_app(SIBLING, Some(OWN), "whoami", json!({}))
        .await;
    assert_eq!(
        sibling.status,
        StatusCode::NOT_FOUND,
        "another app: {}",
        sibling.raw
    );

    let path = format!("/customer-apps/{}/{}", s.agent.t.org_slug, s.agent.app.slug);
    let (bearer, host) = (s.agent.bearer(), s.sandbox_host.as_str());
    let on_host: &[(&str, &str)] = &[("authorization", &bearer), ("host", host)];
    let with_header: &[(&str, &str)] = &[
        ("authorization", &bearer),
        ("host", host),
        ("x-oxy-app-env", OWN),
    ];
    for headers in [on_host, with_header] {
        let status = serve("POST", &format!("{path}/fn/whoami"), headers).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a subdomain host: {headers:?}"
        );
    }
    let product: &[(&str, &str)] = &[("authorization", &bearer), ("x-oxy-app-env", OWN)];
    for bundle in [format!("{path}/"), format!("{path}/index.html")] {
        let status = serve("GET", &bundle, product).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {bundle}");
    }
}

#[tokio::test]
async fn everything_outside_its_own_sandbox_is_refused_and_nothing_moves() {
    let s = scene().await;
    let app = super::fixture::app_model(&s.agent.t.db, s.agent.app.id).await;
    let before = sandbox_rows(&s.agent.t.db, app.id).await;

    other_environments_of_its_app_do_not_exist(&s).await;
    a_publish_reaches_only_its_own_sandbox(&s).await;
    nothing_else_on_the_api_exists(&s).await;
    a_credential_is_not_stored_as_a_secret(&s).await;
    the_public_reads_are_closed(&s).await;
    fn_runs_only_in_its_own_sandbox(&s).await;

    assert_eq!(
        sandbox_rows(&s.agent.t.db, app.id).await,
        before,
        "no sandbox was created, moved or marked deleting"
    );
    let after = super::fixture::app_model(&s.agent.t.db, app.id).await;
    assert_eq!(
        (after.published_build_id, after.draft_build_id),
        (app.published_build_id, app.draft_build_id),
        "production and staging never moved"
    );
    let colleague = app_environments::Entity::find_by_id((app.id, COLLEAGUES.to_string()))
        .one(&s.agent.t.db)
        .await
        .expect("read the colleague's sandbox")
        .expect("still there");
    assert_eq!((colleague.build_id, colleague.deleting_at), (None, None));
    let sibling = super::fixture::app_model(&s.agent.t.db, s.sibling.id).await;
    assert_eq!(sibling.draft_build_id, s.sibling.draft_build_id);
}
