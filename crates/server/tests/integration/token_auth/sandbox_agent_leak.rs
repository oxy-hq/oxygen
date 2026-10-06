//! Leak revocation covers the sandbox agent kind (sandbox agent credential
//! design §2 "Revoked by", §7.3 phase 3): a reported `oxy_sbx_` token is
//! revoked like any new-format token, recorded on its granted app's org, its
//! minter is told, and the token is refused on its very next request.

use axum::http::StatusCode;
use oxy_app::emails::token_mail::outbox;
use oxy_auth::token::format::{SCANNER_PATTERN, generate_sandbox_agent};
use serde_json::{Value, json};

use super::sandbox_agent::{minted, staff_with_app};
use super::stack::{flat_api, get_as};
use super::{audit_rows, call, pat_row};

async fn report(body: Value) -> (StatusCode, Value) {
    call(
        flat_api(),
        "POST",
        "/auth/tokens/revoke-leaked",
        &[],
        Some(body),
    )
    .await
}

#[tokio::test]
async fn a_leaked_sandbox_agent_token_is_revoked_recorded_and_stops_on_its_next_request() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_APP_EMAIL_LOCAL_TEST", "1");
        std::env::set_var("OXY_API_URL", "https://oxy.example");
    }
    let (fx, app) = staff_with_app().await;
    let (id, secret) = minted(&fx, &[app.id]).await;
    assert!(SCANNER_PATTERN.contains("sbx"), "a scanner looks for it");
    assert_eq!(get_as(&secret, "/auth/token").await.0, StatusCode::OK);
    let prefix = pat_row(&fx.db, id).await.display_prefix;
    assert!(prefix.starts_with("oxy_sbx_"), "{prefix}");

    // A lookalike and a stranger's well-formed token are `unknown`, and the
    // real one is still alive after them.
    let forged = format!(
        "{}{}",
        &secret[..secret.len() - 1],
        if secret.ends_with('a') { 'b' } else { 'a' }
    );
    let stranger = generate_sandbox_agent().plaintext;
    let (status, answer) = report(json!([{ "token": forged }, { "token": stranger }])).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer[0]["status"], "unknown", "{answer}");
    assert_eq!(answer[1]["status"], "unknown", "{answer}");
    assert!(pat_row(&fx.db, id).await.revoked_at.is_none());

    let body = json!([{ "token": secret, "source": "github", "url": "https://github.com/acme/agent/blob/main/.env" }]);
    let (status, answer) = report(body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!([{ "display_prefix": prefix, "status": "revoked" }])
    );
    let revoked = pat_row(&fx.db, id).await;
    assert!(revoked.revoked_at.is_some());
    assert_eq!(revoked.revoke_reason.as_deref(), Some("leaked"));
    assert_eq!(revoked.revoked_by, None, "no person revoked it");
    // No sleep and no cache window: this kind is read on every request.
    assert_eq!(
        get_as(&secret, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED
    );

    let audit = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].actor_type, "system");
    assert_eq!(audit[0].org_id, Some(fx.org_id), "the granted app's org");
    assert_eq!(audit[0].target_id, Some(id.to_string()));
    assert_eq!(audit[0].metadata["token_kind"], "sandbox_agent");
    assert_eq!(audit[0].metadata["reason"], "leaked");
    assert_eq!(audit[0].metadata["leak_source"], "github");
    assert!(
        !audit[0].metadata.to_string().contains(&secret),
        "never the token"
    );

    let mail = outbox();
    assert_eq!(mail.len(), 1, "the minter is told");
    assert_eq!(Some(mail[0].to.clone()), fx.user.email);
    assert!(!mail[0].text_body.contains(&secret));

    // Reported again: nothing changes, nothing is recorded or mailed twice.
    let (_, answer) = report(body).await;
    assert_eq!(answer[0]["status"], "already_revoked");
    assert_eq!(audit_rows(&fx.db, "token.revoked").await.len(), 1);
    assert_eq!(outbox().len(), 1);
}
