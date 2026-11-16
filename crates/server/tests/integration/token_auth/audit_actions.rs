//! "What did this token do?" (design §3.7, layer 2): an audited action
//! performed with a token says so, the same action from a session does not,
//! and neither breaks the org's hash chain.
//!
//! These read as legacy-key tests until the keys were removed. The contract
//! never belonged to legacy keys — it is the same for every credential — so
//! the vehicle is now a personal access token, and the lifecycle event in the
//! chain comes from `/user/tokens/{id}/extend` rather than the `/api-keys`
//! route that went with them.

use super::stack::{Reach, flat_api, mint, org_grant};
use super::*;
use axum::http::StatusCode;
use oxy_app_core::audit::AuditFilter;
use oxy_authz::RoleCeiling;

async fn audited_as(fx: &Fixture, headers: &[(&str, &str)]) -> StatusCode {
    let uri = format!("/{}/audited", fx.workspace_id);
    call(audited_surface(fx), "GET", &uri, headers, None)
        .await
        .0
}

/// Push out a personal token's expiry in the fixture's session — one audited
/// `token.extended` row, in the same org chain.
async fn extend_in_session(fx: &Fixture, id: Uuid, body: Value) {
    let uri = format!("/user/tokens/{id}/extend");
    let (status, extended) = call(
        flat_api(),
        "POST",
        &uri,
        &[("cookie", &fx.cookie)],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{extended}");
}

#[tokio::test]
async fn an_action_performed_with_a_token_records_api_key_and_the_token_id() {
    let fx = fixture().await;
    let (first_id, first) = minted_pat(&fx, None).await;
    let (pat_id, pat) = minted_pat(&fx, None).await;
    let bearer = format!("Bearer {pat}");

    let headers = [
        ("x-api-key", first.as_str()),
        ("x-forwarded-for", "203.0.113.4"),
    ];
    assert_eq!(audited_as(&fx, &headers).await, StatusCode::OK);
    // Two hops: the caller wrote the first, the load balancer appended the second.
    let spoofing = [
        ("authorization", bearer.as_str()),
        ("x-forwarded-for", "6.6.6.6, 198.51.100.7"),
    ];
    assert_eq!(audited_as(&fx, &spoofing).await, StatusCode::OK);

    let rows = audit_rows(&fx.db, AUDITED_ACTION).await;
    assert_eq!(rows.len(), 2);
    for (row, id, kind, name) in [
        (&rows[0], first_id, "personal", "minted"),
        (&rows[1], pat_id, "personal", "minted"),
    ] {
        assert_eq!(row.actor_type, "api_key");
        assert_eq!(row.actor_user_id, Some(fx.user.id));
        assert_eq!(row.metadata["token_id"], json!(id));
        assert_eq!(row.metadata["token_kind"], kind);
        assert_eq!(row.metadata["token_name"], name);
        assert!(
            row.metadata["display_prefix"]
                .as_str()
                .unwrap()
                .starts_with("oxy_")
        );
        // The handler's own metadata survives beside the stamp.
        assert_eq!(row.metadata["surface"], "test");
    }
    assert_eq!(rows[0].ip.as_deref(), Some("203.0.113.4"));
    assert_eq!(
        rows[1].ip.as_deref(),
        Some("198.51.100.7"),
        "the hop the load balancer appended, not the one the caller wrote"
    );
    // Never the credential itself, in any column.
    let everything = format!("{rows:?}");
    assert!(!everything.contains(&first) && !everything.contains(&pat));
}

#[tokio::test]
async fn the_same_action_from_a_session_records_a_user_and_no_token_id() {
    let fx = fixture().await;
    assert_eq!(
        audited_as(&fx, &[("cookie", &fx.cookie)]).await,
        StatusCode::OK
    );
    let rows = audit_rows(&fx.db, AUDITED_ACTION).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_type, "user");
    assert_eq!(rows[0].actor_user_id, Some(fx.user.id));
    assert!(rows[0].metadata.get("token_id").is_none());
    assert!(rows[0].metadata.get("token_kind").is_none());
    assert_eq!(rows[0].metadata["surface"], "test");
}

#[tokio::test]
async fn token_stamped_rows_keep_the_org_hash_chain_intact() {
    // jsonb hands object keys back sorted by length, then bytes. A row hashed
    // in any other order verifies as tampered, so every entry is written in
    // that order — including the four stamp keys and nested objects.
    let fx = fixture().await;
    let (_, key) = minted_pat(&fx, None).await;
    assert_eq!(
        audited_as(&fx, &[("x-api-key", &key)]).await,
        StatusCode::OK
    );
    assert_eq!(
        audited_as(&fx, &[("cookie", &fx.cookie)]).await,
        StatusCode::OK
    );
    // A token lifecycle event in the same chain (multi-key metadata and change).
    // Granted this org, so its event is written in this org's chain: a token's
    // lifecycle event is recorded once per org it reaches, and an all-access
    // token with no grant reaches no org by a grant.
    let (target, _) = mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Owner)]),
    )
    .await;
    extend_in_session(&fx, target, json!({ "days": 30 })).await;

    let report = audit::verify_chain(&fx.db, fx.org_id)
        .await
        .expect("verify");
    assert_eq!(report.events, 3);
    assert!(report.intact, "chain broke: {:?}", report.detail);
}

#[tokio::test]
async fn the_admin_audit_token_filter_finds_actions_and_lifecycle_events() {
    let fx = fixture().await;
    // Granted this org, so the lifecycle event below lands in its chain and
    // the admin filter has both kinds of row to find.
    let granted = || Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Owner)]);
    let (id, key) = mint(&fx.db, fx.user.id, granted()).await;
    let (other_id, other) = mint(&fx.db, fx.user.id, granted()).await;
    assert_eq!(
        audited_as(&fx, &[("x-api-key", &key)]).await,
        StatusCode::OK
    );
    assert_eq!(
        audited_as(&fx, &[("x-api-key", &other)]).await,
        StatusCode::OK
    );
    assert_eq!(
        audited_as(&fx, &[("cookie", &fx.cookie)]).await,
        StatusCode::OK
    );
    extend_in_session(&fx, id, json!({ "expires_at": null })).await;

    let filter = AuditFilter {
        token_id: Some(id),
        ..Default::default()
    };
    let found = audit::search_events(&fx.db, &filter, 50, 0).await.unwrap();
    let actions: Vec<&str> = found.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        ["token.extended", AUDITED_ACTION],
        "newest first: the lifecycle event (by target) and the action (by token_id)"
    );
    assert!(
        found
            .iter()
            .all(|e| e.metadata["token_id"] != json!(other_id))
    );
}
