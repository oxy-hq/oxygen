//! Minting an org **service account's token** through `oxyc` (`oxyc tokens
//! create --org-service-account`): the PKCE exchange of `oxyc login`, carrying a
//! `mint` of `kind: "service_account"` — and the device-code twin.
//!
//! The browser approves under its session, as an owner or admin of the org
//! the mint names; the CLI, holding no credential, redeems the code for an
//! `oxy_sat_`. The account is reused or created and the token minted the way
//! the org's API access page does both (`service_accounts`). What these tests
//! hold:
//!
//! - the token reaches the org's admin routes when the account is an admin;
//! - only someone who may manage the org's service accounts can approve;
//! - an existing account at another role is refused, never changed;
//! - a token still cannot create an account or mint a token, directly or by
//!   approving a mint.

use axum::http::StatusCode;
use entity::org_members::{self, OrgRole};
use entity::service_accounts;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::agent_token::{VERIFIER, authorize, exchange, secret_of};
use super::device_login::{approve as approve_request, poll_with, started};
use super::sandbox_agent::{another_session, assert_refused};
use super::service_accounts::{admin_fixture, create_account, in_session, with_token};
use super::stack::join_org;
use super::{Fixture, audit_rows, minted_pat};

const HOST: &str = "ops-laptop";
const ACCOUNT: &str = "store-ops-tablets";

/// The org's slug, as an operator names it on the command line.
async fn slug_of(db: &DatabaseConnection, org_id: Uuid) -> String {
    entity::organizations::Entity::find_by_id(org_id)
        .one(db)
        .await
        .unwrap()
        .expect("the org")
        .slug
}

/// A `mint` for `ACCOUNT` in the fixture's org, with `extra` over it.
async fn account_mint(fx: &Fixture, extra: Value) -> Value {
    let mut mint = json!({
        "kind": "service_account",
        "org": slug_of(&fx.db, fx.org_id).await,
        "account": ACCOUNT,
    });
    if let (Value::Object(mint), Value::Object(extra)) = (&mut mint, extra) {
        mint.extend(extra);
    }
    mint
}

/// Approve `mint` under `cookie` and redeem the code: the exchange's body.
async fn minted(cookie: &str, mint: Value) -> Value {
    let (status, approved) = authorize(cookie, HOST, Some(mint)).await;
    assert_eq!(status, StatusCode::OK, "authorize: {approved}");
    let code = approved["code"].as_str().expect("a code");
    let (status, minted) = exchange(code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "exchange: {minted}");
    minted
}

async fn accounts_named(
    db: &DatabaseConnection,
    org_id: Uuid,
    name: &str,
) -> Vec<service_accounts::Model> {
    service_accounts::Entity::find()
        .filter(service_accounts::Column::OrgId.eq(org_id))
        .filter(service_accounts::Column::Name.eq(name))
        .all(db)
        .await
        .unwrap()
}

fn devices_uri(org_id: Uuid) -> String {
    format!("/orgs/{org_id}/frontline/devices")
}

#[tokio::test]
async fn an_approved_admin_mint_creates_the_account_and_its_token_passes_org_admin_routes() {
    let fx = admin_fixture().await;
    let minted = minted(
        &fx.cookie,
        account_mint(&fx, json!({ "role": "admin" })).await,
    )
    .await;

    let secret = secret_of(&minted);
    assert!(secret.starts_with("oxy_sat_"), "{secret}");
    let token = &minted["token"];
    assert_eq!(token["kind"], "service_account");
    assert_eq!(token["source"], "oxyc");
    assert_eq!(token["name"], format!("minted by oxyc on {HOST}"));
    assert_eq!(token["all_access"], false);
    assert_eq!(token["owner"]["label"], ACCOUNT);
    // No grant was named: one org-wide grant at the account's role.
    let grants = token["grants"].as_array().expect("grants");
    assert_eq!(grants.len(), 1, "{token}");
    assert_eq!(grants[0]["org_id"], json!(fx.org_id));
    assert_eq!(grants[0]["workspace_id"], Value::Null);
    assert_eq!(grants[0]["role_ceiling"], "admin");

    // The account was created, as an admin, with its own audit row.
    let accounts = accounts_named(&fx.db, fx.org_id, ACCOUNT).await;
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].org_role, "admin");
    assert_eq!(accounts[0].created_by, Some(fx.user.id));
    let created = audit_rows(&fx.db, "service_account.created").await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].actor_user_id, Some(fx.user.id));

    // The token's own row names the approver and the asking host.
    let rows = audit_rows(&fx.db, "token.created").await;
    let row = rows
        .iter()
        .find(|row| row.metadata["token_id"] == token["id"])
        .expect("the token's created row");
    assert_eq!(row.org_id, Some(fx.org_id));
    assert_eq!(row.actor_user_id, Some(fx.user.id));
    assert_eq!(row.metadata["hostname"], HOST);
    assert_eq!(row.metadata["service_account"], ACCOUNT);

    // An `OrgAdmin` route — the kiosk list Store Ops' tablets function calls.
    let (status, body) = with_token(&secret, "GET", &devices_uri(fx.org_id), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_member_account_is_the_default_and_its_token_stops_at_member() {
    let fx = admin_fixture().await;
    let minted = minted(&fx.cookie, account_mint(&fx, json!({})).await).await;
    assert_eq!(minted["token"]["grants"][0]["role_ceiling"], "member");
    assert_eq!(
        accounts_named(&fx.db, fx.org_id, ACCOUNT).await[0].org_role,
        "member"
    );
    let (status, _) = with_token(&secret_of(&minted), "GET", &devices_uri(fx.org_id), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_existing_account_is_reused_at_its_role_and_never_changed() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, ACCOUNT, "member").await;

    // Asked for as an admin: refused, and the account is left a member.
    let asked_admin = account_mint(&fx, json!({ "role": "admin" })).await;
    let (status, body) = authorize(&fx.cookie, HOST, Some(asked_admin.clone())).await;
    assert_refused(
        (status, body.clone()),
        StatusCode::CONFLICT,
        "role_mismatch",
        "an existing member asked for as an admin",
    );
    assert_eq!(body["account"], ACCOUNT);
    assert_eq!(body["role"], "member");
    let (status, _) = in_session(
        &fx.cookie,
        "POST",
        "/auth/cli/service-account/preview",
        Some(json!({ "hostname": HOST, "mint": asked_admin })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "the preview says the same");
    let accounts = accounts_named(&fx.db, fx.org_id, ACCOUNT).await;
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].org_role, "member");

    // Asked for at its role: the same account mints, and no second is made.
    let minted = minted(
        &fx.cookie,
        account_mint(&fx, json!({ "role": "member" })).await,
    )
    .await;
    assert_eq!(minted["token"]["owner"]["id"], json!(sa_id));
    assert_eq!(accounts_named(&fx.db, fx.org_id, ACCOUNT).await.len(), 1);
    assert_eq!(audit_rows(&fx.db, "service_account.created").await.len(), 1);
}

#[tokio::test]
async fn a_disabled_account_is_refused_rather_than_handed_a_dead_token() {
    let fx = admin_fixture().await;
    let sa_id = create_account(&fx, ACCOUNT, "admin").await;
    let uri = format!("/orgs/{}/service-accounts/{sa_id}", fx.org_id);
    let (status, _) =
        in_session(&fx.cookie, "PATCH", &uri, Some(json!({ "disabled": true }))).await;
    assert_eq!(status, StatusCode::OK);

    let mint = account_mint(&fx, json!({ "role": "admin" })).await;
    assert_refused(
        authorize(&fx.cookie, HOST, Some(mint)).await,
        StatusCode::CONFLICT,
        "service_account_disabled",
        "a disabled account",
    );
}

#[tokio::test]
async fn only_an_owner_or_admin_of_the_org_may_approve() {
    let fx = admin_fixture().await;
    let mint = account_mint(&fx, json!({ "role": "admin" })).await;

    // A member of the org: refused, no code issued.
    let (member, member_cookie) = another_session(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    assert_refused(
        authorize(&member_cookie, HOST, Some(mint.clone())).await,
        StatusCode::FORBIDDEN,
        "org_admin_required",
        "a plain member",
    );

    // Someone outside the org: the org reads as not there, by slug or by id.
    let (_, outsider_cookie) = another_session(&fx.db, "outsider").await;
    assert_refused(
        authorize(&outsider_cookie, HOST, Some(mint.clone())).await,
        StatusCode::NOT_FOUND,
        "org_not_found",
        "a non-member",
    );
    let mut by_id = mint.clone();
    by_id["org"] = json!(fx.org_id.to_string());
    assert_refused(
        authorize(&outsider_cookie, HOST, Some(by_id)).await,
        StatusCode::NOT_FOUND,
        "org_not_found",
        "a non-member naming the org by id",
    );
    let mut missing = mint.clone();
    missing["org"] = json!("no-such-org");
    assert_refused(
        authorize(&fx.cookie, HOST, Some(missing)).await,
        StatusCode::NOT_FOUND,
        "org_not_found",
        "an org that does not exist",
    );

    // An org admin who is not its owner may.
    let (admin, admin_cookie) = another_session(&fx.db, "admin").await;
    join_org(&fx.db, fx.org_id, admin.id, OrgRole::Admin).await;
    let token = minted(&admin_cookie, mint).await;
    assert_eq!(token["token"]["kind"], "service_account");

    // Nothing was created by the refusals.
    assert_eq!(audit_rows(&fx.db, "service_account.created").await.len(), 1);
}

#[tokio::test]
async fn an_approver_who_lost_the_org_before_the_exchange_mints_nothing() {
    let fx = admin_fixture().await;
    let mint = account_mint(&fx, json!({ "role": "admin" })).await;
    let (status, approved) = authorize(&fx.cookie, HOST, Some(mint)).await;
    assert_eq!(status, StatusCode::OK, "{approved}");

    // Demoted to member in the five minutes the code lives.
    org_members::Entity::update_many()
        .col_expr(org_members::Column::Role, Expr::value(OrgRole::Member))
        .filter(org_members::Column::OrgId.eq(fx.org_id))
        .filter(org_members::Column::UserId.eq(fx.user.id))
        .exec(&fx.db)
        .await
        .unwrap();

    let (status, body) = exchange(approved["code"].as_str().unwrap(), VERIFIER).await;
    assert_refused(
        (status, body),
        StatusCode::BAD_REQUEST,
        "invalid_code",
        "an approval the approver can no longer make",
    );
    assert!(accounts_named(&fx.db, fx.org_id, ACCOUNT).await.is_empty());
}

#[tokio::test]
async fn a_token_cannot_create_an_account_mint_a_token_or_approve_a_mint() {
    let fx = admin_fixture().await;
    let sa_token = secret_of(
        &minted(
            &fx.cookie,
            account_mint(&fx, json!({ "role": "admin" })).await,
        )
        .await,
    );
    let sa_id = accounts_named(&fx.db, fx.org_id, ACCOUNT).await[0].user_id;
    // The owner's own all-access personal token, with staff standing.
    let (_, owner_token) = minted_pat(&fx, None).await;

    let accounts = format!("/orgs/{}/service-accounts", fx.org_id);
    let tokens = format!("{accounts}/{sa_id}/tokens");
    for (who, secret) in [
        ("the admin account's token", &sa_token),
        ("the owner's token", &owner_token),
    ] {
        assert_refused(
            with_token(
                secret,
                "POST",
                &accounts,
                Some(json!({ "name": "next-bot", "org_role": "admin" })),
            )
            .await,
            StatusCode::FORBIDDEN,
            "session_required",
            &format!("{who} creating an account"),
        );
        assert_refused(
            with_token(
                secret,
                "POST",
                &tokens,
                Some(json!({ "name": "successor" })),
            )
            .await,
            StatusCode::FORBIDDEN,
            "session_required",
            &format!("{who} minting a token"),
        );
        // Nor may it approve the mint that would do both.
        let body = json!({
            "code_challenge": oxy_auth::token::cli_login::challenge_of(VERIFIER),
            "hostname": HOST,
            "mint": account_mint(&fx, json!({ "role": "admin" })).await,
        });
        assert_refused(
            with_token(secret, "POST", "/auth/cli/authorize", Some(body)).await,
            StatusCode::FORBIDDEN,
            "session_required",
            &format!("{who} approving a mint"),
        );
        let preview = json!({ "hostname": HOST, "mint": account_mint(&fx, json!({})).await });
        assert_refused(
            with_token(
                secret,
                "POST",
                "/auth/cli/service-account/preview",
                Some(preview),
            )
            .await,
            StatusCode::FORBIDDEN,
            "session_required",
            &format!("{who} previewing a mint"),
        );
    }
    assert!(
        accounts_named(&fx.db, fx.org_id, "next-bot")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn the_preview_is_what_approving_would_create_and_writes_nothing() {
    let fx = admin_fixture().await;
    let slug = slug_of(&fx.db, fx.org_id).await;
    let mint = account_mint(
        &fx,
        json!({ "role": "admin", "name": "tablets", "expires_in_days": 30 }),
    )
    .await;
    let (status, preview) = in_session(
        &fx.cookie,
        "POST",
        "/auth/cli/service-account/preview",
        Some(json!({ "hostname": HOST, "mint": mint })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["org"]["id"], json!(fx.org_id));
    assert_eq!(preview["org"]["slug"], slug);
    assert_eq!(
        preview["account"],
        json!({ "name": ACCOUNT, "role": "admin", "exists": false })
    );
    assert_eq!(preview["token"]["name"], "tablets");
    assert_eq!(preview["token"]["expires_in_days"], 30);
    assert!(preview["token"]["expires_at"].is_string(), "{preview}");
    assert_eq!(
        preview["grant"],
        json!({ "org_wide": true, "role_ceiling": "admin" })
    );
    assert!(accounts_named(&fx.db, fx.org_id, ACCOUNT).await.is_empty());

    // A member is refused here as at the approval.
    let (member, cookie) = another_session(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Member).await;
    let (status, body) = in_session(
        &cookie,
        "POST",
        "/auth/cli/service-account/preview",
        Some(json!({ "hostname": HOST, "mint": account_mint(&fx, json!({})).await })),
    )
    .await;
    assert_refused(
        (status, body),
        StatusCode::FORBIDDEN,
        "org_admin_required",
        "a member's preview",
    );
}

#[tokio::test]
async fn a_device_code_request_mints_the_same_token() {
    let fx = admin_fixture().await;
    let slug = slug_of(&fx.db, fx.org_id).await;
    let ask = json!({
        "kind": "service_account",
        "org": slug,
        "account": ACCOUNT,
        "role": "admin",
        "days": "30",
    });
    let (device_code, user_code) = started(Some(ask)).await;

    // The approval must be of the kind asked for.
    let agent = json!({ "kind": "agent", "expires_in_hours": 2 });
    let (status, _) = approve_request(&fx.cookie, &user_code, Some(agent)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mint = account_mint(&fx, json!({ "role": "admin", "expires_in_days": 30 })).await;
    let (status, body) = approve_request(&fx.cookie, &user_code, Some(mint)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, polled) = poll_with(&device_code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{polled}");
    assert_eq!(polled["status"], "approved");
    let secret = secret_of(&polled);
    assert!(secret.starts_with("oxy_sat_"), "{secret}");

    let rows = audit_rows(&fx.db, "token.created").await;
    let row = rows
        .iter()
        .find(|row| row.metadata["token_id"] == polled["token"]["id"])
        .expect("the token's created row");
    assert_eq!(row.metadata["approval"], "device_code");
    assert_eq!(row.metadata["approver"], json!(fx.user.id));

    let (status, _) = with_token(&secret, "GET", &devices_uri(fx.org_id), None).await;
    assert_eq!(status, StatusCode::OK);
}
