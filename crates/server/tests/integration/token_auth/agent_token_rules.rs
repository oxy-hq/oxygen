//! What holds for an **agent token** once it exists, and who may approve one
//! (API-tokens design, "The agent token (2026-10-07)"; minting is in
//! `agent_token`).
//!
//! - **A credential never mints or approves a credential.** Neither a token
//!   presented as a bearer nor the browser session a token opened may call
//!   `authorize`, for an agent mint or a sandbox agent mint.
//! - It never retires a login, and a login never retires it.
//! - It is fixed at mint: never edited, extended or regenerated, only revoked.
//! - Otherwise it is an ordinary all-access personal token: an org's lifetime
//!   cap and its refusal of all-access tokens fall on it exactly as on one
//!   made in Settings, and it may open a browser session that is itself.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::cli_auth_codes;
use entity::org_members::OrgRole;
use oxy_auth::token::browser_session;
use oxy_auth::token::cli_login::challenge_of;
use oxy_auth::token::credential::source;
use oxy_auth::token::personal::{self, NewToken};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::agent_token::{
    VERIFIER, agent_mint, authorize, expiry_of, id_of, minted, redeemed, secret_of,
};
use super::browser_session::{in_token_session, session_of};
use super::sandbox_agent::{assert_refused, staff_with_app};
use super::service_accounts::{admin_fixture, in_session, with_token, workspace_status};
use super::stack::{Reach, flat_api, get_as, join_org, mint as mint_pat};
use super::{Fixture, audit_rows, call, fixture, seed_org, seed_workspace_in};

/// What `authorize` may be asked for: a mint, or with `None` a login.
type Ask = (&'static str, Option<Value>);

/// `POST /auth/cli/authorize` with `header` as the only credential it carries.
async fn authorize_carrying(header: (&str, &str), mint: Option<&Value>) -> (StatusCode, Value) {
    let mut body = json!({ "code_challenge": challenge_of(VERIFIER), "hostname": "host" });
    if let Some(mint) = mint {
        body["mint"] = mint.clone();
    }
    let uri = "/auth/cli/authorize";
    call(flat_api(), "POST", uri, &[header], Some(body)).await
}

/// `secret` is refused every ask, however it is carried: as itself in either
/// header, and as the browser session it opened, in the header or the cookie.
async fn assert_never_approves(whose: &str, secret: &str, asks: &[Ask]) {
    let jwt = session_of(secret).await;
    // The session is live: what refuses it below is not a bad session.
    let (status, me) = in_token_session(&jwt, "GET", "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{whose}'s session: {me}");
    let (bearer, cookie) = (format!("Bearer {secret}"), format!("oxy_session={jwt}"));
    let carried = [
        ("as a bearer", ("authorization", bearer.as_str())),
        ("as X-API-Key", ("x-api-key", secret)),
        ("as its session's JWT", ("authorization", jwt.as_str())),
        ("as its session's cookie", ("cookie", cookie.as_str())),
    ];
    for (how, header) in carried {
        for (what, mint) in asks {
            let refused = authorize_carrying(header, mint.as_ref()).await;
            let why = format!("{whose} {how} asking for {what}");
            assert_refused(refused, StatusCode::FORBIDDEN, "session_required", &why);
        }
    }
}

/// The codes in this test's database: every row that is not a browser ticket.
async fn codes_issued(db: &DatabaseConnection) -> usize {
    let ticket = |row: &cli_auth_codes::Model| {
        let kind = row.mint.as_ref().and_then(|mint| mint["kind"].as_str());
        kind == Some("browser_session")
    };
    let rows = cli_auth_codes::Entity::find().all(db).await.unwrap();
    rows.iter().filter(|row| !ticket(row)).count()
}

#[tokio::test]
async fn a_credential_never_approves_a_mint_as_a_bearer_or_as_the_session_it_opened() {
    // Staff, with an app: a session of this user may approve all three, so a
    // refusal below is about what is asking and nothing else.
    let (fx, app) = staff_with_app().await;
    let sandbox = json!({ "kind": "sandbox_agent", "apps": [app.id] });
    let asks: [Ask; 3] = [
        (
            "an agent mint",
            Some(agent_mint(json!({ "standing": true }))),
        ),
        ("a sandbox agent mint", Some(sandbox)),
        ("a login", None),
    ];
    for (what, mint) in &asks {
        let (status, body) = authorize(&fx.cookie, "host", mint.clone()).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the session approves {what}: {body}"
        );
    }

    // The credentials that try: an all-access token carrying staff standing,
    // and an agent token carrying it — which would be approving its successor.
    let (_, pat) = mint_pat(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    let agent = secret_of(&minted(&fx.cookie, "host", json!({ "standing": true })).await);
    let issued = codes_issued(&fx.db).await;
    assert_eq!(issued, 4, "the session's three, and the agent token's own");

    assert_never_approves("a personal token", &pat, &asks).await;
    assert_never_approves("an agent token", &agent, &asks).await;
    assert_eq!(
        codes_issued(&fx.db).await,
        issued,
        "no credential was issued a code"
    );
}

#[tokio::test]
async fn an_agent_token_and_a_login_never_retire_each_other() {
    let fx = fixture().await;
    let login = secret_of(&redeemed(&fx.cookie, "laptop", None).await);
    // Several agents on one host each hold a token of the same name, and one
    // was even named as the login is.
    let agents = [
        minted(&fx.cookie, "laptop", json!({})).await,
        minted(&fx.cookie, "laptop", json!({})).await,
        minted(&fx.cookie, "laptop", json!({ "name": "oxyc on laptop" })).await,
    ]
    .map(|minted| secret_of(&minted));
    for secret in agents.iter().chain([&login]) {
        assert_eq!(get_as(secret, "/auth/token").await.0, StatusCode::OK);
    }
    assert!(
        audit_rows(&fx.db, "token.revoked").await.is_empty(),
        "minting an agent token retired nothing"
    );

    // Logging in again from the host retires the earlier login, and only it.
    let again = secret_of(&redeemed(&fx.cookie, "laptop", None).await);
    assert_eq!(
        get_as(&login, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED,
        "the login's own rule still holds"
    );
    for secret in agents.iter().chain([&again]) {
        assert_eq!(get_as(secret, "/auth/token").await.0, StatusCode::OK);
    }
    assert_eq!(audit_rows(&fx.db, "token.revoked").await.len(), 1);
}

#[tokio::test]
async fn nothing_edits_extends_or_regenerates_one() {
    let fx = fixture().await;
    let agent = minted(&fx.cookie, "build-box", json!({})).await;
    let (id, secret) = (id_of(&agent), secret_of(&agent));
    let uri = format!("/user/tokens/{id}");
    for (method, path, body) in [
        ("PATCH", uri.clone(), Some(json!({ "name": "renamed" }))),
        ("PATCH", uri.clone(), Some(json!({ "platform": true }))),
        ("POST", format!("{uri}/extend"), Some(json!({ "days": 30 }))),
        (
            "POST",
            format!("{uri}/extend"),
            Some(json!({ "expires_at": null })),
        ),
        ("POST", format!("{uri}/regenerate"), None),
    ] {
        let refused = in_session(&fx.cookie, method, &path, body.clone()).await;
        let why = format!("{method} {path} {body:?}");
        assert_refused(refused, StatusCode::CONFLICT, "agent_token_fixed", &why);
    }

    // Nothing changed, and the secret it was minted with still works.
    let (status, after) = in_session(&fx.cookie, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["name"], "agent on build-box");
    assert_eq!(after["platform"], false);
    assert_eq!(
        expiry_of(&after).timestamp(),
        expiry_of(&agent["token"]).timestamp()
    );
    assert_eq!(get_as(&secret, "/auth/token").await.0, StatusCode::OK);
    for event in [
        "token.extended",
        "token.regenerated",
        "token.grants_changed",
    ] {
        assert!(audit_rows(&fx.db, event).await.is_empty(), "{event}");
    }

    // The same routes still edit a personal token made in Settings.
    let made = json!({ "name": "laptop" });
    let (status, made) = in_session(&fx.cookie, "POST", "/user/tokens", Some(made)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let uri = format!("/user/tokens/{}", id_of(&made));
    let rename = Some(json!({ "name": "desk" }));
    let (status, renamed) = in_session(&fx.cookie, "PATCH", &uri, rename).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    let (extend, month) = (format!("{uri}/extend"), Some(json!({ "days": 30 })));
    let (status, extended) = in_session(&fx.cookie, "POST", &extend, month).await;
    assert_eq!(status, StatusCode::OK, "{extended}");
}

#[tokio::test]
async fn its_owner_revokes_it_in_a_session_and_it_may_revoke_itself() {
    let fx = fixture().await;
    let by_owner = minted(&fx.cookie, "build-box", json!({})).await;
    let by_itself = minted(&fx.cookie, "build-box", json!({})).await;
    for agent in [&by_owner, &by_itself] {
        assert_eq!(
            get_as(&secret_of(agent), "/auth/token").await.0,
            StatusCode::OK
        );
    }

    let uri = format!("/user/tokens/{}", id_of(&by_owner));
    let (status, _) = in_session(&fx.cookie, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = with_token(&secret_of(&by_itself), "DELETE", "/auth/token", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    for agent in [&by_owner, &by_itself] {
        let (status, _) = get_as(&secret_of(agent), "/auth/token").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let reasons: Vec<Value> = audit_rows(&fx.db, "token.revoked")
        .await
        .iter()
        .map(|row| row.metadata["reason"].clone())
        .collect();
    assert_eq!(reasons, [json!("owner"), json!("self")]);
    // Revoked, not deleted: its owner still sees both.
    let (_, list) = in_session(&fx.cookie, "GET", "/user/tokens", None).await;
    let listed = list["tokens"].as_array().expect("tokens");
    assert_eq!(listed.len(), 2, "{list}");
    assert!(listed.iter().all(|t| t["status"] == "revoked"), "{list}");
}

/// A second org the fixture's user owns, with a workspace.
async fn second_workspace(fx: &Fixture) -> Uuid {
    let org = seed_org(&fx.db).await;
    join_org(&fx.db, org, fx.user.id, OrgRole::Owner).await;
    seed_workspace_in(&fx.db, org).await
}

async fn put_policy(fx: &Fixture, max_lifetime_days: Option<i64>, allow_all_access: bool) {
    let body = json!({
        "max_lifetime_days": max_lifetime_days,
        "allow_all_access_tokens": allow_all_access,
        "require_environment_on_trust_policies": true,
    });
    let uri = format!("/orgs/{}/token-policy", fx.org_id);
    let (status, saved) = in_session(&fx.cookie, "PUT", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "put policy: {saved}");
}

/// An all-access personal token made the way Settings makes one.
async fn made_in_settings(fx: &Fixture, body: Value) -> Value {
    let (status, made) = in_session(&fx.cookie, "POST", "/user/tokens", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    made
}

async fn reaches(token: &Value, workspace: Uuid) -> StatusCode {
    workspace_status(&secret_of(token), workspace, "GET", "read").await
}

#[tokio::test]
async fn an_orgs_lifetime_cap_falls_on_it_as_on_an_all_access_token_made_in_settings() {
    let fx = admin_fixture().await;
    let elsewhere = second_workspace(&fx).await;
    put_policy(&fx, Some(1), true).await;

    // Two days against a one-day cap, both ways of making the token. Neither
    // is refused: an all-access token goes inert where it is capped.
    let settings = made_in_settings(&fx, json!({ "name": "settings", "expires_in_days": 2 })).await;
    let agent = minted(&fx.cookie, "build-box", json!({ "expires_in_hours": 48 })).await;
    let capped = json!([{ "org_id": fx.org_id, "org_name": "Acme", "reason": "max_lifetime" }]);
    for (how, token) in [("settings", &settings), ("agent", &agent)] {
        assert_eq!(token["token"]["blocked_orgs"], capped, "{how}");
        assert_eq!(
            reaches(token, fx.workspace_id).await,
            StatusCode::NOT_FOUND,
            "{how}: inert in the org whose cap it outlives"
        );
        assert_eq!(
            reaches(token, elsewhere).await,
            StatusCode::NO_CONTENT,
            "{how}"
        );
    }

    // Within the cap it works there: the default eight hours.
    let short = minted(&fx.cookie, "build-box", json!({})).await;
    assert_eq!(short["token"]["blocked_orgs"], json!([]));
    assert_eq!(
        reaches(&short, fx.workspace_id).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn an_org_that_refuses_all_access_tokens_blocks_it_as_one_made_in_settings() {
    let fx = admin_fixture().await;
    let elsewhere = second_workspace(&fx).await;
    let earlier = minted(&fx.cookie, "build-box", json!({})).await;
    assert_eq!(
        reaches(&earlier, fx.workspace_id).await,
        StatusCode::NO_CONTENT
    );

    put_policy(&fx, None, false).await;

    // Still minted, both ways — and blocked in that org, both ways.
    let settings = made_in_settings(&fx, json!({ "name": "settings" })).await;
    let agent = minted(&fx.cookie, "build-box", json!({})).await;
    let blocked = json!([{
        "org_id": fx.org_id, "org_name": "Acme", "reason": "all_access_disallowed",
    }]);
    for (how, token) in [("settings", &settings), ("agent", &agent)] {
        assert_eq!(token["token"]["blocked_orgs"], blocked, "{how}");
    }
    for (how, token) in [
        ("earlier", &earlier),
        ("settings", &settings),
        ("agent", &agent),
    ] {
        assert_eq!(
            reaches(token, fx.workspace_id).await,
            StatusCode::NOT_FOUND,
            "{how}"
        );
        assert_eq!(
            reaches(token, elsewhere).await,
            StatusCode::NO_CONTENT,
            "{how}"
        );
    }
}

#[tokio::test]
async fn it_opens_a_browser_session_that_is_the_token_and_ends_with_it() {
    let fx = fixture().await;
    let agent = minted(&fx.cookie, "build-box", json!({ "expires_in_hours": 1 })).await;
    let (id, secret) = (id_of(&agent), secret_of(&agent));

    let jwt = session_of(&secret).await;
    assert_eq!(browser_session::token_id_of(&jwt), Some(id));
    let (status, me) = in_token_session(&jwt, "GET", "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["id"], agent["token"]["id"]);
    assert_eq!(me["source"], "oxyc_agent");

    // The session is the token: revoking the one ends the other.
    let uri = format!("/user/tokens/{id}");
    let (status, _) = in_session(&fx.cookie, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = in_token_session(&jwt, "GET", "/auth/token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_longest_lived_one_is_never_mailed_an_expiry_notice() {
    // The notice asks its owner to extend a token, and nothing extends this
    // one. It is not mailed because it was minted with a life no longer than
    // the notice window — at a week, exactly as long.
    let fx = fixture().await;
    let agent = minted(&fx.cookie, "build-box", json!({ "expires_in_hours": 168 })).await;
    // The control: a token given a day more is due its notice two days on.
    let control = personal::create(
        &fx.db,
        NewToken {
            user_id: fx.user.id,
            name: "eight days".into(),
            all_access: true,
            platform: false,
            partner: false,
            grants: Vec::new(),
            expires_at: Some(Utc::now() + Duration::days(8)),
            source: source::UI,
        },
    )
    .await
    .expect("mint the control");

    let in_two_days = Utc::now() + Duration::days(2);
    let due: Vec<Uuid> = oxy_auth::token::hygiene::due_notices(&fx.db, in_two_days)
        .await
        .expect("read the due notices")
        .iter()
        .map(|token| token.id)
        .collect();
    assert!(due.contains(&control.row.id), "the control is due");
    assert!(!due.contains(&id_of(&agent)), "the agent token is not");
}
