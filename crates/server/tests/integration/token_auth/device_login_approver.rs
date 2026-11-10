//! `oxyc … --no-browser --approver <email>`: the platform tells the named
//! person, and only they may decide the request.
//!
//! The device-code binding itself is `device_login.rs`'s: the person still
//! types the code from the terminal. What the approver adds, tested here:
//!
//! - a start naming a user's email leaves one `oxy.cli_approval` notice for
//!   them, worded from the request, linking to the page without the code;
//! - an email that names nobody starts exactly the same and tells nobody, so
//!   the asker learns nothing about who has an account;
//! - a request bound to an approver answers 404 to anyone else's read,
//!   approve and deny, as for a wrong code, and decides normally for them.
//!
//! A notice queues its delivery as a task, so the tests that expect one run
//! over [`Schema::All`]; the binding tests need only the central schema.

use axum::http::StatusCode;
use entity::notifications;
use entity::org_members::OrgRole;
use oxy_api_notifications::platform::kinds::CLI_APPROVAL;
use oxy_api_notifications::platform::render;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::device_login::{HOST, VERIFIER, approve, describe, page, started};
use super::sandbox_agent::another_session;
use super::stack::{flat_api, join_org};
use super::{Schema, call, fixture, fixture_with};
use oxy_auth::token::cli_login::challenge_of;

/// Start naming `approver`, as `oxyc --approver` does.
async fn start_naming(approver: &str, ask: Option<Value>) -> (StatusCode, Value) {
    let mut body = json!({
        "code_challenge": challenge_of(VERIFIER),
        "hostname": HOST,
        "approver": approver,
    });
    if let Some(ask) = ask {
        body["ask"] = ask;
    }
    call(flat_api(), "POST", "/auth/cli/device", &[], Some(body)).await
}

fn user_code_of(body: &Value) -> String {
    body["user_code"].as_str().expect("user_code").to_string()
}

/// The notice is written off the start's clock (a spawned task), so a read
/// right after the start waits for it: up to two seconds, polling, until
/// `user` has at least `n` notices. What is there then is returned either way.
async fn notices_until(db: &DatabaseConnection, user: Uuid, n: usize) -> Vec<notifications::Model> {
    for _ in 0..20 {
        let found = notices_now(db, user).await;
        if found.len() >= n {
            return found;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    notices_now(db, user).await
}

/// What `user` has after the spawned writes have had their chance: a wait
/// long enough for any of them, for the asserts that something was *not*
/// written.
async fn notices_settled(db: &DatabaseConnection, user: Uuid) -> Vec<notifications::Model> {
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    notices_now(db, user).await
}

async fn notices_now(db: &DatabaseConnection, user: Uuid) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user))
        .filter(notifications::Column::Kind.eq(CLI_APPROVAL))
        .all(db)
        .await
        .unwrap()
}

fn agent_ask() -> Value {
    json!({ "kind": "agent", "hours": "4" })
}

#[tokio::test]
async fn a_start_naming_a_users_email_tells_them_without_the_code() {
    let fx = fixture_with(Schema::All).await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let email = fx.user.email.clone().unwrap();

    let (status, body) = start_naming(&email, Some(agent_ask())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let user_code = user_code_of(&body);

    let told = notices_until(&fx.db, fx.user.id, 1).await;
    assert_eq!(told.len(), 1, "one notice for the approver");
    let notice = &told[0];
    assert_eq!(notice.org_id, fx.org_id);
    assert_eq!(notice.app_id, None, "a platform notice, no app");
    assert_eq!(
        notice.subject_id.as_deref(),
        Some(user_code.replace('-', "").as_str()),
        "the request, by its stored user code"
    );

    // The words are built from the request, not stored.
    let said = render::message(&fx.db, notice).await.unwrap();
    assert_eq!(
        said.title,
        format!("oxyc on {HOST} asks for an agent token (4 h)")
    );
    assert_eq!(said.link.as_deref(), Some("/cli-auth/device"));
    let everything = format!(
        "{} {} {} {}",
        said.title,
        said.body.clone().unwrap_or_default(),
        said.link.clone().unwrap_or_default(),
        said.email_text.clone().unwrap_or_default()
    );
    for code in [user_code.as_str(), &user_code.replace('-', "")] {
        assert!(
            !everything.contains(code),
            "the notice carries the code: {everything}"
        );
    }
    assert!(!everything.contains("?code="), "{everything}");
    assert!(
        said.body
            .unwrap()
            .contains("type the code from the terminal")
    );

    // The address matches in any case, and a second request is a second notice.
    let (status, again) = start_naming(&email.to_uppercase(), None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(notices_until(&fx.db, fx.user.id, 2).await.len(), 2);
}

#[tokio::test]
async fn an_unknown_email_starts_the_same_and_tells_nobody() {
    let fx = fixture_with(Schema::All).await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let email = fx.user.email.clone().unwrap();

    let (status, known) = start_naming(&email, None).await;
    assert_eq!(status, StatusCode::OK, "{known}");
    let (status, unknown) = start_naming("nobody-at-all@example.com", None).await;
    assert_eq!(status, StatusCode::OK, "{unknown}");

    // The asker cannot tell the two apart.
    let shape = |body: &Value| {
        let mut keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        (
            keys,
            body["user_code"].as_str().unwrap().len(),
            body["expires_in"].clone(),
        )
    };
    assert_eq!(shape(&known), shape(&unknown));

    // Only the one that resolved told anybody.
    assert_eq!(notices_until(&fx.db, fx.user.id, 1).await.len(), 1);
    let all = notifications::Entity::find()
        .filter(notifications::Column::Kind.eq(CLI_APPROVAL))
        .all(&fx.db)
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].user_id, fx.user.id);

    // A named address binds the request either way: the one nobody holds can
    // be opened by nobody, so who may open a request tells the asker nothing.
    let (_, stranger) = another_session(&fx.db, "stranger").await;
    let code = user_code_of(&unknown);
    assert_eq!(describe(&stranger, &code).await.0, StatusCode::NOT_FOUND);
    assert_eq!(describe(&fx.cookie, &code).await.0, StatusCode::NOT_FOUND);
}

/// An anonymous caller can aim at most three pending requests' notices at a
/// person; the fourth is stored and bound like the others, and tells nobody.
#[tokio::test]
async fn notices_stop_at_the_pending_cap() {
    let fx = fixture_with(Schema::All).await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let email = fx.user.email.clone().unwrap();
    for _ in 0..4 {
        let (status, body) = start_naming(&email, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert_eq!(notices_until(&fx.db, fx.user.id, 3).await.len(), 3);
    // And no fourth, once every spawned write has had its chance.
    assert_eq!(notices_settled(&fx.db, fx.user.id).await.len(), 3);
}

#[tokio::test]
async fn a_user_in_no_org_is_bound_but_there_is_nowhere_to_file_a_notice() {
    let fx = fixture_with(Schema::All).await;
    let (status, body) = start_naming(fx.user.email.as_deref().unwrap(), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(notices_settled(&fx.db, fx.user.id).await.is_empty());
    let (_, stranger) = another_session(&fx.db, "stranger").await;
    assert_eq!(
        describe(&stranger, &user_code_of(&body)).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_request_bound_to_an_approver_is_decided_only_by_them() {
    let fx = fixture().await;
    let (_, stranger) = another_session(&fx.db, "stranger").await;
    let email = fx.user.email.clone().unwrap();

    // Read and approve.
    let (status, body) = start_naming(&email, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let code = user_code_of(&body);
    assert_eq!(describe(&stranger, &code).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        approve(&stranger, &code, None).await.0,
        StatusCode::NOT_FOUND
    );
    // The stranger decided nothing: it still waits for the approver.
    assert_eq!(describe(&fx.cookie, &code).await.0, StatusCode::OK);
    let (status, decided) = approve(&fx.cookie, &code, None).await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    assert_eq!(decided["status"], "approved");

    // Deny.
    let (_, second) = start_naming(&email, None).await;
    let code = user_code_of(&second);
    let uri = format!("/auth/cli/device/requests/{code}/deny");
    assert_eq!(
        page(&stranger, "POST", &uri, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, decided) = page(&fx.cookie, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{decided}");
    assert_eq!(decided["status"], "denied");
}

#[tokio::test]
async fn an_approver_that_cannot_be_an_email_is_refused_at_the_start() {
    let _fx = fixture().await;
    let too_long = format!("{}@example.com", "a".repeat(250));
    for bad in [
        "not-an-email",
        "a@b@c",
        "two words@example.com",
        "@example.com",
        too_long.as_str(),
    ] {
        let (status, body) = start_naming(bad, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    // A request that names nobody is unchanged.
    let (_, code) = started(None).await;
    assert_eq!(code.len(), 9);
}
