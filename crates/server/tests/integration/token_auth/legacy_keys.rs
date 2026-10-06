//! The regression every API-tokens phase carries (design §3.5): a key created
//! the old way still authenticates, with the same reach and precedence,
//! everywhere it did — and an older pod's revoke still sticks.

use super::*;
use axum::http::StatusCode;
use chrono::Duration;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use sea_orm::ConnectionTrait;

/// `/api` and `/external/api`, by header and by `?api_key=`.
async fn assert_works_everywhere(fx: &Fixture, id: Uuid, key: &str) {
    let query = format!("?api_key={key}");
    for (surface, router) in [("api", api_surface()), ("external", external_surface())] {
        let (status, body) = probe_as(fx, router, "", &[("x-api-key", key)]).await;
        assert_eq!(status, StatusCode::OK, "{surface} X-API-Key: {body}");
        assert_eq!(body["user_id"], json!(fx.user.id));
        assert_eq!(body["token_id"], json!(id));
        assert_eq!(body["kind"], "legacy_key");
    }
    for (surface, router) in [("api", api_surface()), ("external", external_surface())] {
        let (status, _) = probe_as(fx, router, &query, &[]).await;
        assert_eq!(status, StatusCode::OK, "{surface} ?api_key=");
    }
    // The custom-app path: subdomain serving, gates, the data plane.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-api-key", key.parse().unwrap());
    let identity = BuiltInAuthenticator::new()
        .authenticate(&headers)
        .await
        .expect("custom-app path");
    assert_eq!(identity.user_id, Some(fx.user.id));
}

#[tokio::test]
async fn a_key_minted_before_the_migration_resolves_through_its_backfilled_hash() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, None).await;
    // The migration ran against the template before this row existed; run its
    // backfill now, as it ran in production over the keys that existed then.
    fx.db
        .execute_unprepared(migration::API_TOKENS_BACKFILL_SQL)
        .await
        .expect("backfill");
    let row = token_row(&fx.db, id).await.expect("backfilled");
    assert_eq!(row.id, id);
    assert_eq!(row.kind, "legacy_key");
    assert_eq!(row.source, "legacy_backfill");
    assert!(row.all_access && row.platform && row.partner);
    // sha256(key_hash) in SQL must equal the hash Rust looks up by.
    assert_eq!(row.token_hash, oxy_auth::token::hash_token(&key));
    assert_eq!(row.display_prefix, "oxy_");
    assert_eq!(row.last_four, key[key.len() - 4..]);

    assert_works_everywhere(&fx, id, &key).await;

    // Re-running the backfill is a no-op.
    fx.db
        .execute_unprepared(migration::API_TOKENS_BACKFILL_SQL)
        .await
        .expect("idempotent");
}

#[tokio::test]
async fn a_key_an_older_pod_minted_after_the_migration_still_works_and_is_mirrored() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, None).await;
    assert!(token_row(&fx.db, id).await.is_none(), "no row yet");

    assert_works_everywhere(&fx, id, &key).await;

    let row = token_row(&fx.db, id).await.expect("mirrored at first use");
    assert_eq!(row.source, "legacy_lazy");
    assert_eq!(row.token_hash, oxy_auth::token::hash_token(&key));
}

#[tokio::test]
async fn a_legacy_key_also_works_as_a_bearer() {
    // New and strictly additive: an `oxy_<hex>` bearer answered 401 before.
    let fx = fixture().await;
    let (_, key) = legacy_key(&fx, None).await;
    let bearer = format!("Bearer {key}");
    for router in [api_surface(), external_surface()] {
        let (status, _) = probe_as(&fx, router, "", &[("authorization", &bearer)]).await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn a_valid_cookie_still_beats_a_bad_legacy_key() {
    let fx = fixture().await;
    let headers = [
        ("cookie", fx.cookie.as_str()),
        ("x-api-key", "oxy_00000000000000000000000000000000"),
    ];
    let (status, body) = probe_as(&fx, api_surface(), "", &headers).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "today's precedence: the cookie wins"
    );
    assert_eq!(body["user_id"], json!(fx.user.id));
    assert_eq!(body["token_id"], Value::Null);
}

#[tokio::test]
async fn external_api_still_refuses_a_cookie() {
    let fx = fixture().await;
    let (status, _) = probe_as(&fx, external_surface(), "", &[("cookie", &fx.cookie)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_expired_legacy_key_is_refused_as_before() {
    let fx = fixture().await;
    let (_, key) = legacy_key(&fx, Some(Utc::now() - Duration::minutes(1))).await;
    let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_key_revoked_by_the_previous_release_is_refused() {
    let fx = fixture().await;

    // Mirrored (backfilled, used), then revoked by writing api_keys alone.
    let (id, key) = legacy_key(&fx, None).await;
    fx.db
        .execute_unprepared(migration::API_TOKENS_BACKFILL_SQL)
        .await
        .unwrap();
    let (ok, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(ok, StatusCode::OK);
    revoke_in_api_keys_only(&fx.db, id).await;
    oxy_auth::token::cache::clear(); // as another pod, or 30 s later
    for router in [api_surface(), external_surface()] {
        let (status, _) = probe_as(&fx, router, "", &[("x-api-key", &key)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "mirrored key");
    }

    // Never mirrored, revoked the same way.
    let (id, key) = legacy_key(&fx, None).await;
    revoke_in_api_keys_only(&fx.db, id).await;
    let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "unmirrored key");
    assert!(
        token_row(&fx.db, id).await.is_none(),
        "a revoked key is never mirrored as live"
    );
}

#[tokio::test]
async fn using_a_key_records_last_used_in_both_tables() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, None).await;
    assert!(key_row(&fx.db, id).await.last_used_at.is_none());
    let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        key_row(&fx.db, id).await.last_used_at.is_some(),
        "the API Keys page's Last-used column reads this"
    );
    let row = token_row(&fx.db, id).await.expect("mirrored");
    assert!(row.last_used_at.is_some());
}

#[tokio::test]
async fn revoke_over_http_writes_both_tables_and_ends_the_key_at_once() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, None).await;
    let (ok, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(ok, StatusCode::OK); // now cached on this pod

    let uri = format!("/{}/api-keys/{id}", fx.workspace_id);
    let (status, body) = call(
        api_surface(),
        "DELETE",
        &uri,
        &[("cookie", &fx.cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!key_row(&fx.db, id).await.is_active);
    let row = token_row(&fx.db, id).await.expect("mirror");
    assert!(row.revoked_at.is_some());
    assert_eq!(row.revoked_by, Some(fx.user.id));

    // This pod's cache was invalidated on commit: refused without waiting.
    let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_legacy_key_keeps_revoking_but_a_pat_cannot() {
    // Create and revoke accepted any key before this release; a legacy key
    // keeps that (§3.5). A new-format token is refused (§4.6).
    let fx = fixture().await;
    let (_, legacy) = legacy_key(&fx, None).await;
    let (_, pat) = minted_pat(&fx, None).await;
    let (target_a, _) = legacy_key(&fx, None).await;
    let (target_b, _) = legacy_key(&fx, None).await;

    let uri = format!("/{}/api-keys/{target_a}", fx.workspace_id);
    let (status, _) = call(
        api_surface(),
        "DELETE",
        &uri,
        &[("x-api-key", &legacy)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let uri = format!("/{}/api-keys/{target_b}", fx.workspace_id);
    let (status, body) = call(api_surface(), "DELETE", &uri, &[("x-api-key", &pat)], None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        body["error"],
        "revoking an API key requires a browser session"
    );
    assert!(key_row(&fx.db, target_b).await.is_active);
}
