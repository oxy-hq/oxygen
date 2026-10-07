//! A token's sign-in link, end to end: a personal token asks for a one-time
//! ticket, a browser — holding no credential yet — redeems it, and the session
//! it gets **is the token**, in the shape a browser carries.
//!
//! That last part is the property everything here is about. A session that
//! decided as a login would turn a narrow, revocable credential into its
//! owner's whole reach, and let it mint its own successors. So each test asks
//! the session what it is, on the real routes, rather than trusting the
//! payload that came back.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use chrono::{Duration, Utc};
use entity::cli_auth_codes;
use entity::org_members::OrgRole;
use jsonwebtoken::{EncodingKey, Header, encode};
use oxy_auth::token::browser_session::{self, SESSION_TTL_SECS};
use oxy_auth::token::cli_login::challenge_of;
use oxy_authz::RoleCeiling;
use sea_orm::EntityTrait;
use sea_orm::sea_query::Expr;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::stack::{Reach, flat_api, get_as, join_org, mint, workspace_grant};
use super::{audit_rows, call, endpoint_key, fixture, minted_pat, pat_row, seed_org};

const OPENED: &str = "token.browser_session_opened";

/// `POST /auth/browser-ticket` as `secret`.
async fn ticket_as(secret: &str) -> (StatusCode, Value) {
    let bearer = format!("Bearer {secret}");
    call(
        flat_api(),
        "POST",
        "/auth/browser-ticket",
        &[("authorization", &bearer)],
        None,
    )
    .await
}

async fn ticket_for(secret: &str) -> String {
    let (status, body) = ticket_as(secret).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["ticket"].as_str().expect("a ticket").to_string()
}

/// The redemption carries no credential at all, and — unlike [`call`] — this
/// keeps the response headers: the cookie is half of what it answers.
async fn redeem(ticket: &str) -> (StatusCode, HeaderMap, Value) {
    send(
        flat_api(),
        "POST",
        "/auth/browser-ticket/redeem",
        &[],
        Some(json!({ "ticket": ticket })),
    )
    .await
}

async fn send(
    router: Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = router.oneshot(req.body(body).unwrap()).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (parts.status, parts.headers, json)
}

/// The session a fresh ticket of `secret` opens: its JWT.
async fn session_of(secret: &str) -> String {
    let (status, _, body) = redeem(&ticket_for(secret).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().expect("a session").to_string()
}

/// `method uri` as the web app sends it: the bare JWT in `Authorization`.
async fn in_token_session(jwt: &str, method: &str, uri: &str) -> (StatusCode, Value) {
    call(flat_api(), method, uri, &[("authorization", jwt)], None).await
}

fn assert_invalid_ticket((status, headers, body): (StatusCode, HeaderMap, Value), why: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    assert_eq!(body["code"], "invalid_ticket", "{why}: {body}");
    assert!(body.get("token").is_none(), "{why}");
    assert!(headers.get(header::SET_COOKIE).is_none(), "{why}");
}

#[tokio::test]
async fn a_ticket_opens_a_session_that_is_the_token() {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let (token_id, secret) = minted_pat(&fx, None).await;

    let (status, issued) = ticket_as(&secret).await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let ticket = issued["ticket"].as_str().expect("a ticket");
    assert_eq!(
        issued["path"],
        json!(format!("/token-login#ticket={ticket}"))
    );
    assert_eq!(issued["session_seconds"], json!(SESSION_TTL_SECS));

    let (status, headers, opened) = redeem(ticket).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["user"]["id"], json!(fx.user.id.to_string()));
    assert_eq!(opened["orgs"][0]["id"], json!(fx.org_id.to_string()));
    let jwt = opened["token"].as_str().expect("a session");
    assert_eq!(browser_session::token_id_of(jwt), Some(token_id));

    // The cookie carries the same JWT and lapses with it, not in thirty days.
    let cookie = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(
        cookie.starts_with(&format!("oxy_session={jwt};")),
        "{cookie}"
    );
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    let max_age: i64 = cookie
        .split("; ")
        .find_map(|part| part.strip_prefix("Max-Age="))
        .expect("a Max-Age")
        .parse()
        .unwrap();
    assert!(
        max_age > SESSION_TTL_SECS - 60 && max_age <= SESSION_TTL_SECS,
        "{max_age}"
    );

    // What the session is, asked of the route that describes the calling
    // token: a login session answers 404 there. Header and cookie alike.
    let (status, me) = in_token_session(jwt, "GET", "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["id"], json!(token_id));
    let as_cookie = format!("oxy_session={jwt}");
    let (status, me) = call(
        flat_api(),
        "GET",
        "/auth/token",
        &[("cookie", &as_cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["id"], json!(token_id));
}

#[tokio::test]
async fn opening_a_session_is_audited_on_the_token() {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let (token_id, secret) = minted_pat(&fx, None).await;
    session_of(&secret).await;

    let rows = audit_rows(&fx.db, OPENED).await;
    assert_eq!(rows.len(), 1, "one event, in the one org the token reaches");
    assert_eq!(
        rows[0].target_id.as_deref(),
        Some(token_id.to_string().as_str())
    );
    assert_eq!(rows[0].org_id, Some(fx.org_id));
    let metadata = &rows[0].metadata;
    assert_eq!(metadata["token_id"], json!(token_id));
    assert!(metadata["session_expires_at"].is_string(), "{metadata}");
}

#[tokio::test]
async fn a_ticket_works_once() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let ticket = ticket_for(&secret).await;

    assert_eq!(redeem(&ticket).await.0, StatusCode::OK);
    assert_invalid_ticket(redeem(&ticket).await, "a ticket already redeemed");
    assert_invalid_ticket(redeem("no-such-ticket").await, "a ticket never issued");
}

#[tokio::test]
async fn a_form_post_from_another_site_sets_no_cookie_and_spends_nothing() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let ticket = ticket_for(&secret).await;

    // What `<form enctype="text/plain">` can be made to send: valid JSON,
    // under the one content type that needs no CORS preflight.
    let req = Request::builder()
        .method("POST")
        .uri("/auth/browser-ticket/redeem")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(json!({ "ticket": ticket }).to_string()))
        .unwrap();
    let resp = flat_api().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(resp.headers().get(header::SET_COOKIE).is_none());

    // The control: the same ticket, sent as JSON, still opens its session.
    assert_eq!(redeem(&ticket).await.0, StatusCode::OK);
}

#[tokio::test]
async fn an_expired_ticket_opens_nothing() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let ticket = ticket_for(&secret).await;
    // The only row in this test's database is the ticket's.
    cli_auth_codes::Entity::update_many()
        .col_expr(
            cli_auth_codes::Column::ExpiresAt,
            Expr::value((Utc::now() - Duration::seconds(1)).fixed_offset()),
        )
        .exec(&fx.db)
        .await
        .unwrap();
    assert_invalid_ticket(redeem(&ticket).await, "a ticket past its five minutes");
}

#[tokio::test]
async fn a_ticket_and_a_login_code_are_not_each_others() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

    // A ticket handed to `oxyc login`'s exchange mints no token.
    let ticket = ticket_for(&secret).await;
    let (status, body) = call(
        flat_api(),
        "POST",
        "/auth/cli/exchange",
        &[],
        Some(json!({ "code": ticket, "code_verifier": verifier })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_code");
    // …and was not spent by the attempt: it is not a row that exchange finds.
    assert_eq!(redeem(&ticket).await.0, StatusCode::OK);

    // A login code handed to the redemption opens no session.
    let (status, authorized) = call(
        flat_api(),
        "POST",
        "/auth/cli/authorize",
        &[("cookie", &fx.cookie)],
        Some(json!({ "code_challenge": challenge_of(verifier), "hostname": "laptop" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{authorized}");
    let code = authorized["code"].as_str().unwrap();
    assert_invalid_ticket(redeem(code).await, "a login code");
}

#[tokio::test]
async fn only_a_personal_token_presented_as_a_token_may_ask() {
    let fx = fixture().await;

    // A login session has no token to sign a browser in as.
    let (status, body) = call(
        flat_api(),
        "POST",
        "/auth/browser-ticket",
        &[("cookie", &fx.cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "no_token");

    // A legacy key gains nothing it did not have.
    let (_, key) = endpoint_key(&fx, None).await;
    let (status, body) = call(
        flat_api(),
        "POST",
        "/auth/browser-ticket",
        &[("x-api-key", &key)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "personal_token_required");

    // Nobody at all is 401, before any of that.
    let (status, _) = call(flat_api(), "POST", "/auth/browser-ticket", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_session_cannot_manage_tokens_or_renew_itself() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let jwt = session_of(&secret).await;

    // Session-only routes refuse it exactly as they refuse its token: a
    // credential never mints, lists or approves a credential.
    for (method, uri) in [
        ("GET", "/user/tokens"),
        ("GET", "/user/token-options"),
        ("POST", "/auth/cli/authorize"),
    ] {
        let (status, body) = in_token_session(&jwt, method, uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
        assert_eq!(body["code"], "session_required", "{method} {uri}");
    }

    // And it cannot ask for the ticket that would outlive its twelve hours.
    let (status, body) = in_token_session(&jwt, "POST", "/auth/browser-ticket").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "no_token");
}

/// A person who signs in with Google, GitHub or a magic link and a token
/// session opened from their token are **one account** — one `users` row, found
/// by email by every login and by id by the token. The two sessions live side
/// by side, and each ends without the other noticing.
#[tokio::test]
async fn a_login_and_a_token_session_of_one_person_are_one_account_side_by_side() {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let (token_id, secret) = minted_pat(&fx, None).await;
    let jwt = session_of(&secret).await;
    let login = [("cookie", fx.cookie.as_str())];

    // The same person, whichever way they came in.
    let (_, as_login) = call(flat_api(), "GET", "/user", &login, None).await;
    let (_, as_token_session) = in_token_session(&jwt, "GET", "/user").await;
    assert_eq!(as_login["id"], json!(fx.user.id.to_string()));
    assert_eq!(as_token_session["id"], as_login["id"]);
    assert_eq!(as_token_session["email"], as_login["email"]);
    // And the same orgs: this token is all-access.
    let (_, orgs_login) = call(flat_api(), "GET", "/orgs", &login, None).await;
    let (_, orgs_token) = in_token_session(&jwt, "GET", "/orgs").await;
    assert_eq!(orgs_token, orgs_login);

    // Opening the token session took nothing from the login: it still manages
    // tokens, which the token session never can.
    let (status, listed) = call(flat_api(), "GET", "/user/tokens", &login, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        in_token_session(&jwt, "GET", "/user/tokens").await.0,
        StatusCode::FORBIDDEN
    );

    // The person ends the token from their login. Its session goes with it;
    // their own does not.
    let (status, _) = call(
        flat_api(),
        "DELETE",
        &format!("/user/tokens/{token_id}"),
        &login,
        None,
    )
    .await;
    assert!(status.is_success(), "{status}");
    assert_eq!(
        in_token_session(&jwt, "GET", "/orgs").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _) = call(flat_api(), "GET", "/orgs", &login, None).await;
    assert_eq!(status, StatusCode::OK, "the login outlives the token");
}

#[tokio::test]
async fn revoking_the_token_ends_its_sessions() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let jwt = session_of(&secret).await;
    assert_eq!(
        in_token_session(&jwt, "GET", "/orgs").await.0,
        StatusCode::OK
    );

    let bearer = format!("Bearer {secret}");
    let (status, _) = call(
        flat_api(),
        "DELETE",
        "/auth/token",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(
        in_token_session(&jwt, "GET", "/orgs").await.0,
        StatusCode::UNAUTHORIZED,
        "the session of a revoked token"
    );
    // Nor does its cookie hydrate into anything.
    let as_cookie = format!("oxy_session={jwt}");
    let (status, _) = call(
        flat_api(),
        "GET",
        "/auth/session",
        &[("cookie", &as_cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_token_revoked_after_its_ticket_was_issued_opens_nothing() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let ticket = ticket_for(&secret).await;

    let bearer = format!("Bearer {secret}");
    call(
        flat_api(),
        "DELETE",
        "/auth/token",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_invalid_ticket(redeem(&ticket).await, "a ticket whose token was revoked");
    assert!(audit_rows(&fx.db, OPENED).await.is_empty());
}

#[tokio::test]
async fn a_narrowed_token_opens_a_narrowed_session() {
    let fx = fixture().await;
    let other_org = seed_org(&fx.db).await;
    for org in [fx.org_id, other_org] {
        join_org(&fx.db, org, fx.user.id, OrgRole::Owner).await;
    }
    let grant = workspace_grant(fx.org_id, fx.workspace_id, RoleCeiling::Viewer);
    let (_, secret) = mint(&fx.db, fx.user.id, Reach::granted(vec![grant])).await;

    // The payload names the one org the token touches, not both its owner's.
    let (status, _, opened) = redeem(&ticket_for(&secret).await).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let listed: Vec<&str> = opened["orgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|org| org["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, vec![fx.org_id.to_string()]);

    // And the session reads what the token reads — the same answer, route for
    // route, as the token gets as a bearer.
    let jwt = opened["token"].as_str().unwrap();
    for uri in ["/orgs".to_string(), format!("/orgs/{other_org}/workspaces")] {
        let as_session = in_token_session(jwt, "GET", &uri).await;
        let as_token = get_as(&secret, &uri).await;
        assert_eq!(as_session, as_token, "{uri}");
    }
    let (status, _) = in_token_session(jwt, "GET", &format!("/orgs/{other_org}/workspaces")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an org outside the grant");
}

#[tokio::test]
async fn the_cookie_hydrates_as_the_same_session_and_never_as_a_login() {
    let fx = fixture().await;
    let (token_id, secret) = minted_pat(&fx, None).await;
    let jwt = session_of(&secret).await;

    let as_cookie = format!("oxy_session={jwt}");
    let (status, headers, hydrated) = send(
        flat_api(),
        "GET",
        "/auth/session",
        &[("cookie", &as_cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hydrated}");
    // The same JWT back: a login's cookie is re-minted for thirty days here,
    // which for a token session would be an escalation.
    assert_eq!(hydrated["token"], json!(jwt));
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert_eq!(hydrated["user"]["id"], json!(fx.user.id.to_string()));
    assert_eq!(
        browser_session::token_id_of(hydrated["token"].as_str().unwrap()),
        Some(token_id)
    );
}

#[tokio::test]
async fn a_session_forged_with_the_login_key_is_refused() {
    let fx = fixture().await;
    let (token_id, _) = minted_pat(&fx, None).await;
    let row = pat_row(&fx.db, token_id).await;

    // Everything an outsider can know: the token's id, its owner's id, and
    // the constant that signs login sessions.
    let now = Utc::now().timestamp();
    let claims = json!({
        "sub": row.principal_user_id.to_string(),
        "email": fx.user.email,
        "exp": now + 3600,
        "iat": now,
        "tid": token_id,
    });
    let header = Header {
        kid: Some(format!("tok:{token_id}")),
        ..Header::default()
    };
    let key = EncodingKey::from_secret(oxy_auth::constants::AUTHENTICATION_SECRET_KEY.as_bytes());
    let forged = encode(&header, &claims, &key).unwrap();

    let (status, _) = in_token_session(&forged, "GET", "/auth/token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = in_token_session(&forged, "GET", "/orgs").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_lapsed_session_is_refused_while_its_token_lives() {
    let fx = fixture().await;
    let (token_id, secret) = minted_pat(&fx, None).await;
    let row = pat_row(&fx.db, token_id).await;

    // Minted thirteen hours ago: properly signed, an hour past its twelve.
    let minted_at = Utc::now() - Duration::hours(13);
    let lapsed = browser_session::mint(&row, "a@example.com", minted_at).unwrap();
    let (status, _) = in_token_session(&lapsed.jwt, "GET", "/orgs").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The control: the token itself still works.
    assert_eq!(get_as(&secret, "/orgs").await.0, StatusCode::OK);
}

/// A ticket's row can outlive its token's (a hard delete, a restore): the
/// redemption finds no row to sign for and answers as for any dead ticket.
#[tokio::test]
async fn a_ticket_for_a_token_that_no_longer_exists_opens_nothing() {
    let fx = fixture().await;
    let issued = browser_session::issue(&fx.db, fx.user.id, Uuid::new_v4())
        .await
        .unwrap();
    assert_invalid_ticket(redeem(&issued.ticket).await, "a ticket naming no token row");
}
