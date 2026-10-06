//! `oxy_pat_` tokens: either header authenticates, and a failing one is 401
//! with no fallthrough to the cookie beside it.

use super::*;
use axum::http::StatusCode;
use chrono::Duration;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;

#[tokio::test]
async fn a_pat_authenticates_as_bearer_and_as_x_api_key_on_every_surface() {
    let fx = fixture().await;
    let (id, pat) = minted_pat(&fx, None).await;
    let bearer = format!("Bearer {pat}");

    for (name, value) in [
        ("authorization", bearer.as_str()),
        ("x-api-key", pat.as_str()),
    ] {
        for router in [api_surface(), external_surface()] {
            let (status, body) = probe_as(&fx, router, "", &[(name, value)]).await;
            assert_eq!(status, StatusCode::OK, "{name}: {body}");
            assert_eq!(body["user_id"], json!(fx.user.id));
            assert_eq!(
                body["token_id"],
                json!(id),
                "the credential marker names the token"
            );
            assert_eq!(body["kind"], "personal");
        }
    }

    // The custom-app path (subdomain serving, gates, data plane) goes through
    // BuiltInAuthenticator, which goes through the same dispatch.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", bearer.parse().unwrap());
    let identity = BuiltInAuthenticator::new()
        .authenticate(&headers)
        .await
        .expect("custom-app path");
    assert_eq!(identity.user_id, Some(fx.user.id));
}

#[tokio::test]
async fn a_pat_is_also_accepted_through_api_key_query() {
    let fx = fixture().await;
    let (_, pat) = minted_pat(&fx, None).await;
    let query = format!("?api_key={pat}");
    for router in [api_surface(), external_surface()] {
        let (status, _) = probe_as(&fx, router, &query, &[]).await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn the_cookie_alone_is_a_session_not_a_credential() {
    let fx = fixture().await;
    let (status, body) = probe_as(&fx, api_surface(), "", &[("cookie", &fx.cookie)]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["token_id"], Value::Null);
}

/// Every way a pat can be bad, each sent beside a valid cookie in both
/// headers. None may fall through to the cookie.
async fn assert_refused_despite_cookie(fx: &Fixture, pat: &str, why: &str) {
    let bearer = format!("Bearer {pat}");
    for (name, value) in [("authorization", bearer.as_str()), ("x-api-key", pat)] {
        let headers = [("cookie", fx.cookie.as_str()), (name, value)];
        let (status, _) = probe_as(fx, api_surface(), "", &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{why} via {name}");
    }
}

#[tokio::test]
async fn a_bad_pat_is_401_even_with_a_valid_cookie() {
    let fx = fixture().await;
    // Well-formed and checksummed, but never stored.
    let unknown = oxy_auth::token::generate_personal().plaintext;
    assert_refused_despite_cookie(&fx, &unknown, "unknown").await;
    // Fails its checksum.
    let mut broken = minted_pat(&fx, None).await.1;
    let last = broken.pop().unwrap();
    broken.push(if last == 'A' { 'B' } else { 'A' });
    assert_refused_despite_cookie(&fx, &broken, "bad checksum").await;
}

#[tokio::test]
async fn a_revoked_pat_is_401_even_with_a_valid_cookie() {
    let fx = fixture().await;
    let (id, pat) = minted_pat(&fx, None).await;
    let row = pat_row(&fx.db, id).await;
    oxy_auth::token::personal::revoke(&fx.db, row, fx.user.id, "owner")
        .await
        .expect("revoke");
    assert_refused_despite_cookie(&fx, &pat, "revoked").await;
}

#[tokio::test]
async fn an_expired_pat_is_401_even_with_a_valid_cookie() {
    let fx = fixture().await;
    let (_, pat) = minted_pat(&fx, Some(Utc::now() - Duration::minutes(1))).await;
    assert_refused_despite_cookie(&fx, &pat, "expired").await;
}

#[tokio::test]
async fn the_legacy_endpoint_mints_a_legacy_key_the_previous_release_still_validates() {
    // The legacy endpoint mints a legacy API key — `oxy_<32 hex>`, never an
    // `oxy_pat_`. N-1 looks keys up by plaintext in api_keys, so a key minted
    // here must pass that lookup, or a rollout or revert would break it.
    let fx = fixture().await;
    let (id, key) = endpoint_key(&fx, None).await;
    assert_eq!(
        oxy_auth::token::format::parse_format(&key),
        Some(oxy_auth::token::format::TokenFormat::LegacyKey),
        "the legacy endpoint returns a legacy key: {key}"
    );
    assert!(!key.starts_with("oxy_pat_"));
    let validated = ApiKeyService::validate_api_key(&fx.db, &key, &ApiKeyConfig::default())
        .await
        .expect("plaintext lookup");
    assert_eq!(validated.id, id);

    let row = token_row(&fx.db, id).await.expect("api_tokens mirror row");
    assert_eq!(row.id, id, "one id across both tables");
    assert_eq!(row.kind, "legacy_key");
    assert_eq!(row.source, "legacy_endpoint");
    assert_eq!(row.token_hash, oxy_auth::token::hash_token(&key));
    assert!(row.all_access && row.platform && row.partner);
    assert!(!row.display_prefix.is_empty() && key.starts_with(&row.display_prefix));

    // It authenticates on both surfaces, by either header and by query.
    let bearer = format!("Bearer {key}");
    for router in [api_surface(), external_surface()] {
        for (name, value) in [
            ("x-api-key", key.as_str()),
            ("authorization", bearer.as_str()),
        ] {
            let (status, body) = probe_as(&fx, router.clone(), "", &[(name, value)]).await;
            assert_eq!(status, StatusCode::OK, "{name}: {body}");
            assert_eq!(body["kind"], "legacy_key");
        }
        let (status, _) = probe_as(&fx, router, &format!("?api_key={key}"), &[]).await;
        assert_eq!(status, StatusCode::OK, "?api_key=");
    }

    // And N-1 sees this release's revoke.
    ApiKeyService::revoke_api_key(&fx.db, id, fx.user.id)
        .await
        .unwrap();
    assert!(
        ApiKeyService::validate_api_key(&fx.db, &key, &ApiKeyConfig::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_legacy_endpoint_key_revoked_by_the_previous_release_is_refused() {
    let fx = fixture().await;
    let (id, key) = endpoint_key(&fx, None).await;
    revoke_in_api_keys_only(&fx.db, id).await;
    let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_legacy_mirror_row_asking_for_what_a_legacy_key_never_is_is_refused() {
    // §4.7: a legacy key is all-access with both standings, always. A mirror
    // row that says otherwise is not something this release can enforce, so it
    // fails closed rather than guess.
    let fx = fixture().await;
    for (column, value) in [
        ("platform", "false"),
        ("partner", "false"),
        ("all_access", "false"),
        ("kind", "'service_account'"),
    ] {
        let (id, pat) = endpoint_key(&fx, None).await;
        let (ok, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &pat)]).await;
        assert_eq!(ok, StatusCode::OK);

        use sea_orm::ConnectionTrait;
        fx.db
            .execute_unprepared(&format!(
                "UPDATE api_tokens SET {column} = {value} WHERE id = '{id}'"
            ))
            .await
            .unwrap();
        oxy_auth::token::cache::clear(); // as another pod, or 30 s later
        let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &pat)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{column} = {value}");
    }
}
