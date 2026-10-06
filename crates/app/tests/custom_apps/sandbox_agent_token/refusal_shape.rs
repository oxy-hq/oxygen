//! A sandbox agent token's refusals answer in one JSON shape, on every route
//! of the loop, and no other credential's answer changes
//! (`internal-docs/custom-app-sandboxes.md` §5.6).
//!
//! Each route family is asked twice through the routers production mounts:
//! with the token, and with a credential of its minter's that the same handler
//! answers. The token gets `{code, error, message}` as JSON. The minter gets
//! the body that route has always answered, to the byte.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use oxy_app::server::api::custom_apps_publish::PublishError;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::fixture::{Agent, BOUNDARY, agent, api_router, multipart, published, send, tarball};
use crate::app_environments::seed_sandbox;
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::serve_router;

const OWN: &str = "dev-own";
/// A sandbox a person created.
const COLLEAGUES: &str = "dev-col";
const SIBLING: &str = "sbx-sibling";

/// What a route answered, as it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: StatusCode,
    content_type: Option<String>,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {self:?}"))
    }

    /// The one shape: JSON, the code under both names, a sentence.
    fn assert_refused(&self, status: StatusCode, code: &str, what: &str) {
        assert_eq!(self.status, status, "{what}: {self:?}");
        assert_eq!(
            self.content_type.as_deref(),
            Some("application/json"),
            "{what}: {self:?}"
        );
        let body = self.json();
        assert_eq!(body["code"], code, "{what}: {body}");
        assert_eq!(body["error"], code, "{what}: {body}");
        let message = body["message"].as_str().unwrap_or_default();
        assert!(!message.trim().is_empty(), "{what}: {body}");
    }

    /// Plain text, exactly `text`.
    fn assert_text(&self, status: StatusCode, text: &str, what: &str) {
        assert_eq!(self.status, status, "{what}: {self:?}");
        let plain = self.content_type.as_deref().unwrap_or_default();
        assert!(plain.starts_with("text/plain"), "{what}: {self:?}");
        assert_eq!(self.body, text, "{what}");
    }

    /// JSON, exactly `expected`: no field more, none less.
    fn assert_json(&self, status: StatusCode, expected: Value, what: &str) {
        assert_eq!(self.status, status, "{what}: {self:?}");
        assert_eq!(self.json(), expected, "{what}");
    }

    /// A status and nothing else.
    fn assert_bare(&self, status: StatusCode, what: &str) {
        assert_eq!(self.status, status, "{what}: {self:?}");
        assert_eq!(self.content_type, None, "{what}: {self:?}");
        assert_eq!(self.body, "", "{what}");
    }
}

async fn answered(router: axum::Router, request: Request<Body>) -> Answer {
    let response = router.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("read the body");
    Answer {
        status,
        content_type,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// Who asks: the name and value of the one header that says so.
type Credential<'a> = (&'a str, &'a str);

/// One request to the flat `/api` tree.
async fn api(who: Credential<'_>, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(who.0, who.1);
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    answered(api_router(), request.body(body).expect("request")).await
}

/// `POST /customer-apps/publish` of the app `slug`, with `fields`.
async fn publish(who: Credential<'_>, s: &Scene, slug: &str, fields: &[(&str, &str)]) -> Answer {
    let (workspace, org) = (
        demo_workspace_id().to_string(),
        s.agent.t.org_id.to_string(),
    );
    let mut all = vec![
        ("app", slug),
        ("project", workspace.as_str()),
        ("org_id", org.as_str()),
    ];
    all.extend_from_slice(fields);
    let request = Request::builder()
        .method("POST")
        .uri("/customer-apps/publish")
        .header(who.0, who.1)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(&all, &tarball(slug, "x"))))
        .expect("request");
    answered(api_router(), request).await
}

/// `POST …/fn/whoami` of the app `slug` over the serve route.
async fn call(who: Credential<'_>, s: &Scene, slug: &str, environment: Option<&str>) -> Answer {
    let uri = format!("/customer-apps/{}/{slug}/fn/whoami", s.agent.t.org_slug);
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(who.0, who.1)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(environment) = environment {
        request = request.header("x-oxy-app-env", environment);
    }
    let request = request.body(Body::from("{}")).expect("request");
    answered(serve_router(), request).await
}

/// The token with a sandbox of its own, beside a colleague's and a second app
/// of the same workspace, and its minter's two other credentials.
struct Scene {
    agent: Agent,
    bearer: String,
    /// An all-access personal token of the minter.
    personal: String,
    sibling: Uuid,
}

impl Scene {
    fn token(&self) -> Credential<'_> {
        ("authorization", self.bearer.as_str())
    }

    fn session(&self) -> Credential<'_> {
        ("cookie", self.agent.cookie.as_str())
    }

    fn personal(&self) -> Credential<'_> {
        ("authorization", self.personal.as_str())
    }

    /// `/customer-apps/<the token's app>/<rest>`.
    fn app(&self, rest: &str) -> String {
        format!("/customer-apps/{}/{rest}", self.agent.app.id)
    }

    /// `/customer-apps/<org>/<slug>/logs<query>`.
    fn logs(&self, slug: &str, query: &str) -> String {
        format!(
            "/customer-apps/{}/{slug}/logs{query}",
            self.agent.t.org_slug
        )
    }
}

async fn scene() -> Scene {
    let agent = agent().await;
    let sibling = published(&agent.t, SIBLING).await.id;
    let (status, created) = agent.create(OWN).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    seed_sandbox(&agent.t.db, agent.app.id, COLLEAGUES, agent.t.guest_id).await;

    let cookie = [("cookie", agent.cookie.as_str())];
    let named = Some(json!({ "name": "the minter's own" }));
    let (status, minted) = send("POST", "/user/tokens", &cookie, named).await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let secret = minted["secret"].as_str().expect("the secret");
    Scene {
        bearer: agent.bearer(),
        personal: format!("Bearer {secret}"),
        sibling,
        agent,
    }
}

/// The fence: a route outside the loop, another app's id and an id that names
/// no app are one `404`, the same bytes, so the token cannot tell them apart.
async fn the_fence_answers_one_not_found(s: &Scene) {
    let token = s.token();
    let outside = api(token, "GET", "/customer-apps", None).await;
    outside.assert_refused(StatusCode::NOT_FOUND, "not_found", "outside the loop");

    let another = format!("/customer-apps/{}/environments", s.sibling);
    let nowhere = format!("/customer-apps/{}/environments", Uuid::new_v4());
    let another = api(token, "GET", &another, None).await;
    let nowhere = api(token, "GET", &nowhere, None).await;
    another.assert_refused(StatusCode::NOT_FOUND, "not_found", "another app");
    assert_eq!(another, nowhere, "another app's id and no app's id");
    assert_eq!(another, outside, "and a route outside the loop");
}

/// Sandbox management: the code each refusal already had, now under `code`
/// too. The minter's session keeps `{error, message}`.
async fn sandbox_management(s: &Scene) {
    let (token, session) = (s.token(), s.session());
    let not_found = StatusCode::NOT_FOUND;
    for name in [COLLEAGUES, "dev-nobody", "staging"] {
        let read = api(token, "GET", &s.app(&format!("environments/{name}")), None).await;
        read.assert_refused(not_found, "environment_not_found", name);
    }
    let bad = Some(json!({ "name": "Not A Name" }));
    let refused = api(token, "POST", &s.app("environments"), bad.clone()).await;
    refused.assert_refused(
        StatusCode::BAD_REQUEST,
        "invalid_environment_name",
        "a bad name",
    );

    let missing = api(session, "GET", &s.app("environments/dev-nobody"), None).await;
    let as_before = json!({
        "error": "environment_not_found",
        "message": "this app has no environment dev-nobody",
    });
    missing.assert_json(not_found, as_before, "the minter's session");
    let refused = api(session, "POST", &s.app("environments"), bad).await;
    let body = refused.json();
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_environment_name", "{body}");
    assert!(
        body.get("code").is_none(),
        "no `code` for a session: {body}"
    );
}

/// Verify and read-back: a coded refusal keeps its code, and a bare `404`
/// gets `not_found`. The minter's session keeps `{error, message}`.
async fn verify_and_read_back(s: &Scene) {
    let (token, session) = (s.token(), s.session());
    let not_found = StatusCode::NOT_FOUND;
    for read in [
        "functions",
        "functions?environment=staging",
        "invocations?environment=production",
        "functions/whoami/invocations?environment=dev-col",
    ] {
        let refused = api(token, "GET", &s.app(read), None).await;
        refused.assert_refused(not_found, "environment_not_found", read);
    }
    let typo = api(token, "GET", &s.app("functions?environment=Bogus"), None).await;
    typo.assert_refused(StatusCode::BAD_REQUEST, "invalid_environment", "a typo");
    let run = format!("function-runs/{}", Uuid::new_v4());
    let run = api(token, "GET", &s.app(&run), None).await;
    run.assert_refused(not_found, "not_found", "a run that is not its own");

    let typo = api(session, "GET", &s.app("functions?environment=Bogus"), None).await;
    let as_before = json!({
        "error": "invalid_environment",
        "message": "\"Bogus\" is not an app environment: use \"production\", \"staging\" or \
                    \"dev-<handle>\"",
    });
    typo.assert_json(StatusCode::BAD_REQUEST, as_before, "the minter's session");
}

/// Secrets: text for everyone else, the one shape for the token, with the
/// code its other routes give the same refusal.
async fn secrets(s: &Scene) {
    let (token, session) = (s.token(), s.session());
    let not_found = StatusCode::NOT_FOUND;
    for query in ["", "?environment=staging", "?environment=dev-col"] {
        let read = api(token, "GET", &s.app(&format!("secrets{query}")), None).await;
        read.assert_refused(not_found, "environment_not_found", query);
    }
    let leak = json!({ "key": "LEAK", "value": s.agent.secret, "environment": OWN });
    let refused = api(token, "POST", &s.app("secrets"), Some(leak)).await;
    refused.assert_refused(
        StatusCode::BAD_REQUEST,
        "credential_shaped_value",
        "its own token as a secret",
    );
    let typo = api(token, "GET", &s.app("secrets?environment=Bogus"), None).await;
    typo.assert_refused(StatusCode::BAD_REQUEST, "bad_request", "a typo");

    let typo = api(session, "GET", &s.app("secrets?environment=Bogus"), None).await;
    let said = "environment \"Bogus\" holds no secrets: use \"production\" (the default), \
                \"staging\", or a sandbox's \"dev-<handle>\"";
    typo.assert_text(StatusCode::BAD_REQUEST, said, "the minter's session");
    // The sentence the token is given as JSON is this route's text for anyone else.
    let missing = api(
        session,
        "GET",
        &s.app("secrets?environment=dev-nobody"),
        None,
    )
    .await;
    let said = "this app has no environment dev-nobody";
    missing.assert_text(not_found, said, "the minter's session");
    let own = api(token, "GET", &s.app("secrets?environment=dev-nobody"), None).await;
    own.assert_refused(not_found, "environment_not_found", "no such sandbox");
    assert_eq!(own.json()["message"], said);
}

/// Publish: text for everyone else. For the token, a sandbox that is not its
/// own is `environment_not_found` whether or not it exists.
async fn publishing(s: &Scene) {
    let (token, session) = (s.token(), s.session());
    let slug = s.agent.app.slug.as_str();
    let channel = [("build_id", "to-staging")];
    let refused = publish(token, s, slug, &channel).await;
    refused.assert_refused(
        StatusCode::FORBIDDEN,
        "sandbox_token_refused",
        "a channel publish",
    );
    for environment in [COLLEAGUES, "dev-nobody"] {
        let fields = [("build_id", "elsewhere"), ("environment", environment)];
        let refused = publish(token, s, slug, &fields).await;
        refused.assert_refused(StatusCode::NOT_FOUND, "environment_not_found", environment);
    }
    let fields = [("build_id", "sibling-1"), ("environment", OWN)];
    let another = publish(token, s, SIBLING, &fields).await;
    another.assert_refused(
        StatusCode::NOT_FOUND,
        "environment_not_found",
        "another app",
    );
    let typo = [("build_id", "typo"), ("environment", "Bogus")];
    let refused = publish(token, s, slug, &typo).await;
    refused.assert_refused(StatusCode::BAD_REQUEST, "bad_request", "a typo");

    let fields = [("build_id", "nowhere"), ("environment", "dev-nobody")];
    let missing = publish(session, s, slug, &fields).await;
    let name = "dev-nobody".to_string();
    let said = PublishError::UnknownEnvironment { name }.to_string();
    missing.assert_text(StatusCode::NOT_FOUND, &said, "the minter's session");
}

/// `/logs` names its app by slugs, so every `404` is the same one: the app
/// that is not the token's, the app that does not exist, the environment that
/// is not its own. Anyone else keeps `{"error":"not permitted"}`.
async fn logs(s: &Scene) {
    let (token, personal) = (s.token(), s.personal());
    let slug = s.agent.app.slug.as_str();
    let own_query = format!("?environment={OWN}");
    let reads = [
        s.logs(slug, ""),
        s.logs(slug, "?environment=staging"),
        s.logs(slug, "?environment=dev-col"),
        s.logs(SIBLING, &own_query),
        s.logs("no-such-app", &own_query),
    ];
    let mut answers = Vec::new();
    for read in &reads {
        let refused = api(token, "GET", read, None).await;
        refused.assert_refused(StatusCode::NOT_FOUND, "not_found", read);
        answers.push(refused);
    }
    assert!(
        answers.iter().all(|answer| answer == &answers[0]),
        "one 404, whatever is missing: {answers:?}"
    );

    let unknown = api(personal, "GET", &s.logs("no-such-app", ""), None).await;
    let as_before = json!({ "error": "not permitted" });
    unknown.assert_json(StatusCode::NOT_FOUND, as_before, "a personal token");
}

/// `/fn`, up to its gate: the fence, the app that is not the token's and the
/// sandbox that is not its own are one `404`, the same bytes. Anyone else
/// keeps the bare status.
async fn calling_a_function(s: &Scene) {
    let (token, personal) = (s.token(), s.personal());
    let slug = s.agent.app.slug.as_str();
    let refusals = [
        ("no environment named", call(token, s, slug, None).await),
        ("staging", call(token, s, slug, Some("staging")).await),
        (
            "a colleague's sandbox",
            call(token, s, slug, Some(COLLEAGUES)).await,
        ),
        (
            "no such sandbox",
            call(token, s, slug, Some("dev-nobody")).await,
        ),
        ("another app", call(token, s, SIBLING, Some(OWN)).await),
        (
            "no such app",
            call(token, s, "no-such-app", Some(OWN)).await,
        ),
    ];
    for (what, refused) in &refusals {
        refused.assert_refused(StatusCode::NOT_FOUND, "not_found", what);
        assert_eq!(refused, &refusals[0].1, "{what}: one 404, the same bytes");
    }

    let unknown = call(personal, s, "no-such-app", None).await;
    unknown.assert_bare(StatusCode::NOT_FOUND, "a personal token");
}

#[tokio::test]
async fn the_token_is_refused_in_one_shape_and_nobody_elses_answer_changes() {
    let s = scene().await;
    the_fence_answers_one_not_found(&s).await;
    sandbox_management(&s).await;
    verify_and_read_back(&s).await;
    secrets(&s).await;
    publishing(&s).await;
    logs(&s).await;
    calling_a_function(&s).await;

    // A dead token is a `401`, as every credential gets it: there is no
    // token left to answer in a shape.
    let (status, body) = s.agent.api("DELETE", "/auth/token", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    oxy_auth::token::cache::clear();
    let dead = api(s.token(), "GET", &s.app("environments"), None).await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED, "{dead:?}");
    assert!(!dead.body.contains("\"code\""), "not shaped: {dead:?}");
}
