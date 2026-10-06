//! "What did this key do?" (design §3.7, layer 2): an audited action performed
//! with a key says so, the same action from a session does not, and neither
//! breaks the org's hash chain.

use super::*;
use axum::http::StatusCode;
use oxy_app_core::audit::AuditFilter;

async fn audited_as(fx: &Fixture, headers: &[(&str, &str)]) -> StatusCode {
    let uri = format!("/{}/audited", fx.workspace_id);
    call(audited_surface(fx), "GET", &uri, headers, None)
        .await
        .0
}

#[tokio::test]
async fn an_action_performed_with_a_key_records_api_key_and_the_token_id() {
    let fx = fixture().await;
    let (legacy_id, legacy) = legacy_key(&fx, None).await;
    let (pat_id, pat) = minted_pat(&fx, None).await;
    let bearer = format!("Bearer {pat}");

    let headers = [
        ("x-api-key", legacy.as_str()),
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
        (&rows[0], legacy_id, "legacy_key", "legacy"),
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
    assert!(!everything.contains(&legacy) && !everything.contains(&pat));
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
async fn key_stamped_rows_keep_the_org_hash_chain_intact() {
    // jsonb hands object keys back sorted by length, then bytes. A row hashed
    // in any other order verifies as tampered, so every entry is written in
    // that order — including the four stamp keys and nested objects.
    let fx = fixture().await;
    let (_, key) = legacy_key(&fx, None).await;
    assert_eq!(
        audited_as(&fx, &[("x-api-key", &key)]).await,
        StatusCode::OK
    );
    assert_eq!(
        audited_as(&fx, &[("cookie", &fx.cookie)]).await,
        StatusCode::OK
    );
    // A token lifecycle event in the same chain (multi-key metadata and change).
    let (target, _) = legacy_key(&fx, None).await;
    let uri = format!("/{}/api-keys/{target}/extend", fx.workspace_id);
    let (status, _) = call(
        api_surface(),
        "POST",
        &uri,
        &[("cookie", &fx.cookie)],
        Some(json!({ "days": 30 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let report = audit::verify_chain(&fx.db, fx.org_id)
        .await
        .expect("verify");
    assert_eq!(report.events, 3);
    assert!(report.intact, "chain broke: {:?}", report.detail);
}

#[tokio::test]
async fn a_real_handler_run_with_a_legacy_key_names_that_key() {
    // Revoke is a shipped, audited handler a legacy key may still call.
    let fx = fixture().await;
    let (acting_id, acting) = legacy_key(&fx, None).await;
    let (by_key, _) = legacy_key(&fx, None).await;
    let (by_session, _) = legacy_key(&fx, None).await;

    let uri = format!("/{}/api-keys/{by_key}", fx.workspace_id);
    let (status, _) = call(
        api_surface(),
        "DELETE",
        &uri,
        &[("x-api-key", &acting)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let uri = format!("/{}/api-keys/{by_session}", fx.workspace_id);
    let (status, _) = call(
        api_surface(),
        "DELETE",
        &uri,
        &[("cookie", &fx.cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let rows = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(rows.len(), 2);
    // Performed with a key: the stamp names the key that ACTED; the key acted
    // ON is the target.
    assert_eq!(rows[0].actor_type, "api_key");
    assert_eq!(rows[0].metadata["token_id"], json!(acting_id));
    assert_eq!(rows[0].target_id, Some(by_key.to_string()));
    assert_eq!(rows[0].metadata["api_key_id"], json!(by_key));
    // Performed in a session: a user, and the token it is about.
    assert_eq!(rows[1].actor_type, "user");
    assert_eq!(rows[1].metadata["token_id"], json!(by_session));
}

#[tokio::test]
async fn the_admin_audit_token_filter_finds_actions_and_lifecycle_events() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, None).await;
    let (other_id, other) = legacy_key(&fx, None).await;
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
    let uri = format!("/{}/api-keys/{id}/extend", fx.workspace_id);
    let (status, _) = call(
        api_surface(),
        "POST",
        &uri,
        &[("cookie", &fx.cookie)],
        Some(json!({ "expires_at": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

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
