//! Finding 11 of the #3408 review: a disable and a mint cannot both win
//! (design §3.4, `oxy_auth::token::ci_mint`).
//!
//! The exchange chooses a policy from rows read outside any transaction and
//! mints a moment later. These tests make the two interleavings a race can
//! produce happen every time, without sleeping:
//!
//! - the change commits first — the mint is handed a policy chosen BEFORE the
//!   change and must refuse;
//! - the mint is in flight first — the change must wait for it (asked with
//!   `NOWAIT`, so "has to wait" is an immediate answer), and its revoke then
//!   sees the token the mint committed.
//!
//! And the same for two WRITES that cross: each decides from the row it locks,
//! never from one it read before its transaction — a disable that crosses a
//! re-enable still disables, and a second delete finds the policy gone. There
//! the request is driven until the database shows it waiting on the row.

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::Poll;

use axum::http::StatusCode;
use chrono::Utc;
use oxy_auth::github_oidc::ClaimReject;
use oxy_auth::token::ci_mint;
use oxy_auth::token::exchange::{self, Decision};
use oxy_auth::token::trust_policy::{self, Candidate};
use oxy_auth::token::trust_policy_access::RepoIds;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::oidc::{OWNER_ID, REPO_ID, Run, policies_uri, whole_org};
use super::service_accounts::in_session;
use super::trusted_access::{ci_tokens, deployer, policy_row, token_row};
use super::{Fixture, audit_rows};

/// The policy an exchange for `run` would choose for `sa`, as it is read
/// before the mint's transaction begins — what the mint is handed.
async fn chosen_for(fx: &Fixture, sa: Uuid, run: &Run) -> Candidate {
    let ids = RepoIds {
        repository_id: REPO_ID,
        repository_owner_id: OWNER_ID,
    };
    let candidates = trust_policy::candidates(&fx.db, sa, ids).await.unwrap();
    match exchange::decide(&run.claims(), candidates, true) {
        Decision::Mint(candidate) => *candidate,
        Decision::Reject(reject) => panic!("the fixture run should be admitted: {reject:?}"),
    }
}

/// The mint itself, in a transaction of its own that is committed — exactly
/// what the exchange runs once it has chosen.
async fn mint_from(fx: &Fixture, chosen: &Candidate, run: &Run) -> Result<Uuid, ClaimReject> {
    let txn = fx.db.begin().await.unwrap();
    let outcome = ci_mint::mint_for_policy(&txn, chosen, &run.claims(), Utc::now())
        .await
        .expect("no database fault");
    txn.commit().await.unwrap();
    outcome.map(|minted| minted.minted.row.id)
}

async fn patch(fx: &Fixture, uri: &str, body: Value) {
    let (status, answer) = in_session(&fx.cookie, "PATCH", uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

/// Finding 11, one half: a change that commits BEFORE the mint is seen by it.
/// The policy is chosen while it would mint, changed, and only then minted
/// from — the interleaving a race produces, made to happen every time.
#[tokio::test]
async fn a_policy_or_account_changed_after_it_was_chosen_mints_nothing() {
    let (fx, sa, policy) = deployer(json!([whole_org("member")])).await;
    let one = format!("{}/{policy}", policies_uri(fx.org_id, sa));
    let account = format!("/orgs/{}/service-accounts/{sa}", fx.org_id);
    let run = Run::new();
    let chosen = chosen_for(&fx, sa, &run).await;

    // Unchanged, what was chosen mints: the control for everything below.
    let minted = mint_from(&fx, &chosen, &run).await.expect("it mints");
    assert_eq!(
        token_row(&fx.db, minted).await.trust_policy_id,
        Some(policy)
    );

    // The policy is disabled after it was chosen. The mint reads it again,
    // under lock, and refuses — as if it had never been a candidate.
    patch(&fx, &one, json!({ "disabled": true })).await;
    assert_eq!(
        mint_from(&fx, &chosen, &run).await,
        Err(ClaimReject::NoMatchingPolicy)
    );
    patch(&fx, &one, json!({ "disabled": false })).await;

    // So is one whose account was disabled in between.
    patch(&fx, &account, json!({ "disabled": true })).await;
    assert_eq!(
        mint_from(&fx, &chosen, &run).await,
        Err(ClaimReject::NoMatchingPolicy)
    );
    patch(&fx, &account, json!({ "disabled": false })).await;

    // An edit is judged, not skipped: the policy now names an environment
    // this run is not in, so it no longer admits the run it was chosen for.
    patch(&fx, &one, json!({ "environment": "staging" })).await;
    assert_eq!(
        mint_from(&fx, &chosen, &run).await,
        Err(ClaimReject::NoMatchingPolicy)
    );
    patch(&fx, &one, json!({ "environment": "production" })).await;
    mint_from(&fx, &chosen, &run)
        .await
        .expect("restored, it mints");

    // Deleted, it is gone for the mint too.
    let (status, _) = in_session(&fx.cookie, "DELETE", &one, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        mint_from(&fx, &chosen, &run).await,
        Err(ClaimReject::NoMatchingPolicy)
    );

    // Two mints, both from before any refusal — and nothing live is left: the
    // disable, the edit and the delete each revoked what there was.
    let tokens = ci_tokens(&fx.db).await;
    assert_eq!(tokens.len(), 2, "a refusal writes nothing");
    assert!(tokens.iter().all(|t| t.revoked_at.is_some()));
}

/// Whether another transaction could take, right now, the lock an `UPDATE` of
/// this row needs. Asked with `NOWAIT`, so the answer is immediate either way.
async fn could_update(db: &DatabaseConnection, table: &str, key: &str, id: Uuid) -> bool {
    let other = db.begin().await.unwrap();
    let sql = format!("SELECT 1 FROM {table} WHERE {key} = $1 FOR NO KEY UPDATE NOWAIT");
    let statement = Statement::from_sql_and_values(DbBackend::Postgres, sql, [id.into()]);
    let free = other.query_one_raw(statement).await.is_ok();
    other.rollback().await.unwrap();
    free
}

/// Finding 11, the other half: a change that arrives WHILE a mint is in flight
/// has to wait for it — and so, when it runs, its revoke sees the token.
#[tokio::test]
async fn a_disable_waits_for_a_mint_in_flight_and_then_revokes_its_token() {
    let (fx, sa, policy) = deployer(json!([whole_org("member")])).await;
    let one = format!("{}/{policy}", policies_uri(fx.org_id, sa));
    let run = Run::new();
    let chosen = chosen_for(&fx, sa, &run).await;
    assert!(could_update(&fx.db, "oidc_trust_policies", "id", policy).await);
    assert!(could_update(&fx.db, "service_accounts", "user_id", sa).await);

    // The mint, minted and not yet committed.
    let mint = fx.db.begin().await.unwrap();
    let minted = ci_mint::mint_for_policy(&mint, &chosen, &run.claims(), Utc::now())
        .await
        .unwrap()
        .expect("it mints")
        .minted
        .row
        .id;

    // It holds both rows: nothing can disable the policy or its account until
    // it is done. No waiting to find out — the lock is asked for with NOWAIT.
    assert!(
        !could_update(&fx.db, "oidc_trust_policies", "id", policy).await,
        "a disable of the policy has to wait for the mint"
    );
    assert!(
        !could_update(&fx.db, "service_accounts", "user_id", sa).await,
        "a disable of the account has to wait for the mint"
    );
    // Another policy of the same account is not held up, nor is the token
    // visible to anyone yet.
    assert!(ci_tokens(&fx.db).await.is_empty());

    // The disable is sent while the mint is still open, and the mint commits.
    // Whichever reaches the database first, the disable's revoke runs after
    // the mint's commit: it cannot get past the policy's lock before that.
    let body = json!({ "disabled": true });
    let (answer, committed) = tokio::join!(
        in_session(&fx.cookie, "PATCH", &one, Some(body)),
        mint.commit()
    );
    committed.unwrap();
    assert_eq!(answer.0, StatusCode::OK, "{}", answer.1);

    // Both happened, and they did not both win: the token exists, and it is
    // revoked — with the policy, not fifteen minutes later.
    let row = token_row(&fx.db, minted).await;
    assert_eq!(row.trust_policy_id, Some(policy));
    assert!(row.revoked_at.is_some(), "the disable saw the minted token");
    assert!(policy_row(&fx.db, policy).await.disabled_at.is_some());
    // And the locks are let go.
    assert!(could_update(&fx.db, "oidc_trust_policies", "id", policy).await);
    assert!(could_update(&fx.db, "service_accounts", "user_id", sa).await);
}

/// How many sessions of this test's database are waiting for a lock right now.
async fn waiting_for_a_lock(db: &DatabaseConnection) -> i64 {
    let sql = "SELECT count(*) AS waiting FROM pg_stat_activity \
               WHERE datname = current_database() AND wait_event_type = 'Lock'";
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .expect("a count")
        .try_get::<i64>("", "waiting")
        .unwrap()
}

/// Drive `requests` until `blocked` of them are waiting on a row lock — which
/// is as far as any of them can get while the test holds it. No sleep: the
/// requests are polled, and the database is asked, until it is so. A request
/// that finishes instead did not wait, and that is the failure.
async fn until_blocked<F>(db: &DatabaseConnection, requests: &mut [Pin<&mut F>], blocked: i64)
where
    F: Future<Output = (StatusCode, Value)>,
{
    loop {
        for request in requests.iter_mut() {
            let early = poll_fn(|cx| Poll::Ready(request.as_mut().poll(cx))).await;
            assert!(
                early.is_pending(),
                "a request did not wait for the row lock: {early:?}"
            );
        }
        if waiting_for_a_lock(db).await >= blocked {
            return;
        }
        tokio::task::yield_now().await;
    }
}

/// Finding 11, the stale read: a write decides from the row it LOCKS, not from
/// one it read before its transaction.
///
/// Off, on, off in quick succession. The second "off" arrives while the "on" is
/// still uncommitted, so anything it read before taking the lock says the
/// policy is disabled. Judged against that, it would find nothing to do:
/// change nothing, revoke nothing, answer 200 — for a policy that is enabled
/// and minting. Judged against the row under the lock, it disables it.
#[tokio::test]
async fn a_disable_that_crosses_a_re_enable_still_disables_and_revokes() {
    let (fx, sa, policy) = deployer(json!([whole_org("member")])).await;
    let one = format!("{}/{policy}", policies_uri(fx.org_id, sa));
    let run = Run::new();
    let chosen = chosen_for(&fx, sa, &run).await;
    patch(&fx, &one, json!({ "disabled": true })).await;

    // "On": a transaction re-enables the policy and holds the row. A run mints
    // in it — a token minted while the policy was enabled.
    let on = fx.db.begin().await.unwrap();
    let held = ci_mint::lock_policy(&on, policy).await.unwrap();
    assert!(held.is_some_and(|row| row.disabled_at.is_some()));
    on.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE oidc_trust_policies SET disabled_at = NULL WHERE id = $1",
        [policy.into()],
    ))
    .await
    .unwrap();
    let minted = ci_mint::mint_for_policy(&on, &chosen, &run.claims(), Utc::now())
        .await
        .unwrap()
        .expect("enabled in this transaction, it mints")
        .minted
        .row
        .id;

    // "Off" again: the request starts, and blocks on the row. Whatever it read
    // before that, it read while the policy still looked disabled.
    let off = in_session(&fx.cookie, "PATCH", &one, Some(json!({ "disabled": true })));
    tokio::pin!(off);
    until_blocked(&fx.db, &mut [off.as_mut()], 1).await;

    // "On" commits: the policy is enabled, and the token is live.
    on.commit().await.unwrap();
    let (status, answer) = off.await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    // The disable was the last word, and it took effect: the stored policy is
    // disabled — not merely reported so — and what was minted in between is
    // revoked with it.
    assert!(
        policy_row(&fx.db, policy).await.disabled_at.is_some(),
        "the stored policy ends disabled"
    );
    assert!(!answer["disabled_at"].is_null(), "{answer}");
    assert!(
        token_row(&fx.db, minted).await.revoked_at.is_some(),
        "the token minted while it was enabled is revoked"
    );
    assert_eq!(
        mint_from(&fx, &chosen, &run).await,
        Err(ClaimReject::NoMatchingPolicy),
        "and it mints nothing more"
    );
}

/// Two deletes of one policy that cross: the second finds it gone under the
/// lock and answers not found — it does not write a second `deleted` row for a
/// policy that was already deleted.
#[tokio::test]
async fn a_second_delete_of_a_policy_is_not_found_and_audited_once() {
    let (fx, sa, policy) = deployer(json!([whole_org("member")])).await;
    let one = format!("{}/{policy}", policies_uri(fx.org_id, sa));

    // Both deletes start while the row is held, so both are past whatever they
    // read up front before either can go on.
    let hold = fx.db.begin().await.unwrap();
    assert!(ci_mint::lock_policy(&hold, policy).await.unwrap().is_some());
    let first = in_session(&fx.cookie, "DELETE", &one, None);
    let second = in_session(&fx.cookie, "DELETE", &one, None);
    tokio::pin!(first, second);
    until_blocked(&fx.db, &mut [first.as_mut(), second.as_mut()], 2).await;
    hold.rollback().await.unwrap();

    let (first, second) = tokio::join!(first, second);
    let mut statuses = [first.0, second.0];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::NO_CONTENT, StatusCode::NOT_FOUND],
        "one deletes it, the other finds it gone: {} / {}",
        first.1,
        second.1
    );
    assert_eq!(
        audit_rows(&fx.db, "trust_policy.deleted").await.len(),
        1,
        "deleted once, audited once"
    );

    // And once it is gone, a delete is simply not found.
    let (status, _) = in_session(&fx.cookie, "DELETE", &one, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(audit_rows(&fx.db, "trust_policy.deleted").await.len(), 1);
}
