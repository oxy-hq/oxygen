//! `POST /api/{workspace_id}/api-keys/{id}/extend` (design §3.6, contract in
//! the Phase 1 PR): session only, owner only, revives an expired key, never a
//! revoked one, writes both tables, and audits old and new expiry.

use super::*;
use axum::http::StatusCode;
use chrono::Duration;
use entity::audit_events;

fn extend_uri(fx: &Fixture, id: Uuid) -> String {
    format!("/{}/api-keys/{id}/extend", fx.workspace_id)
}

async fn extend_as(
    fx: &Fixture,
    id: Uuid,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, Value) {
    call(
        api_surface(),
        "POST",
        &extend_uri(fx, id),
        headers,
        Some(body),
    )
    .await
}

#[tokio::test]
async fn extend_revives_an_expired_key_in_both_tables() {
    let fx = fixture().await;
    let lapsed = Utc::now() - Duration::days(2);
    let (id, key) = legacy_key(&fx, Some(lapsed)).await;
    let (refused, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(
        refused,
        StatusCode::UNAUTHORIZED,
        "expired before the extension"
    );

    let (status, body) = extend_as(&fx, id, &[("cookie", &fx.cookie)], json!({ "days": 30 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], json!(id.to_string()));
    assert_eq!(body["is_active"], true);

    // Counted from now, not from the lapsed expiry.
    let new_expiry: DateTime<Utc> = key_row(&fx.db, id).await.expires_at.unwrap().into();
    let expected = Utc::now() + Duration::days(30);
    assert!(
        (new_expiry - expected).num_seconds().abs() < 60,
        "{new_expiry}"
    );
    let mirror: DateTime<Utc> = token_row(&fx.db, id)
        .await
        .expect("mirror")
        .expires_at
        .unwrap()
        .into();
    assert_eq!(mirror, new_expiry, "both tables agree");

    // The same secret works again; nothing was redistributed.
    let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &key)]).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn extend_is_audited_with_the_old_and_new_expiry_and_never_the_key() {
    let fx = fixture().await;
    let before = Utc::now() + Duration::days(1);
    let (id, key) = legacy_key(&fx, Some(before)).await;
    let (status, _) = extend_as(
        &fx,
        id,
        &[("cookie", &fx.cookie)],
        json!({ "expires_at": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        key_row(&fx.db, id).await.expires_at.is_none(),
        "never expires"
    );

    let row = audit_events::Entity::find()
        .filter(audit_events::Column::Action.eq("token.extended"))
        .filter(audit_events::Column::TargetId.eq(id.to_string()))
        .one(&fx.db)
        .await
        .unwrap()
        .expect("token.extended row");
    assert_eq!(row.target_type.as_deref(), Some("api_token"));
    assert_eq!(row.org_id, Some(fx.org_id));
    assert_eq!(row.workspace_id, Some(fx.workspace_id));
    assert_eq!(row.actor_user_id, Some(fx.user.id));
    let old = row.before.expect("before")["expires_at"].clone();
    assert!(old.as_str().is_some(), "old expiry recorded: {old}");
    assert_eq!(row.after.expect("after")["expires_at"], Value::Null);
    // The activity endpoint reads these from metadata (contract amendment).
    assert_eq!(row.metadata["token_id"], json!(id));
    assert_eq!(row.metadata["old_expires_at"], old);
    assert_eq!(row.metadata["new_expires_at"], Value::Null);
    let everything = format!(
        "{:?} {}",
        row.metadata,
        row.target_label.unwrap_or_default()
    );
    assert!(
        !everything.contains(&key),
        "an audit row never carries the key"
    );
}

#[tokio::test]
async fn extend_refuses_a_token_authenticated_caller() {
    let fx = fixture().await;
    let (legacy_id, legacy) = legacy_key(&fx, None).await;
    let (pat_id, pat) = minted_pat(&fx, None).await;
    let bearer = format!("Bearer {pat}");
    for (id, header) in [
        (legacy_id, ("x-api-key", legacy.as_str())),
        (pat_id, ("x-api-key", pat.as_str())),
        (pat_id, ("authorization", bearer.as_str())),
    ] {
        let (status, body) = extend_as(&fx, id, &[header], json!({ "days": 30 })).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{header:?}");
        assert_eq!(
            body,
            json!({ "error": "extend requires a browser session" })
        );
    }
    // Not even with a cookie alongside: the key decides the request.
    let (status, _) = extend_as(
        &fx,
        pat_id,
        &[("cookie", &fx.cookie), ("x-api-key", &pat)],
        json!({ "days": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn extend_answers_409_for_a_revoked_key() {
    let fx = fixture().await;
    let (id, _) = legacy_key(&fx, None).await;
    ApiKeyService::revoke_api_key(&fx.db, id, fx.user.id)
        .await
        .unwrap();
    let (status, _) = extend_as(&fx, id, &[("cookie", &fx.cookie)], json!({ "days": 30 })).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Revoked by the previous release (api_keys alone) reads the same.
    let (id, _) = legacy_key(&fx, None).await;
    revoke_in_api_keys_only(&fx.db, id).await;
    let (status, _) = extend_as(&fx, id, &[("cookie", &fx.cookie)], json!({ "days": 30 })).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn extend_answers_404_for_someone_elses_key_and_400_for_a_bad_body() {
    let fx = fixture().await;
    let other = seed_user(&fx.db, "other").await;
    let other_fx = Fixture {
        db: fx.db.clone(),
        user: other,
        org_id: fx.org_id,
        workspace_id: fx.workspace_id,
        cookie: String::new(),
    };
    let (theirs, _) = legacy_key(&other_fx, None).await;
    let (status, _) = extend_as(
        &fx,
        theirs,
        &[("cookie", &fx.cookie)],
        json!({ "days": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = extend_as(
        &fx,
        Uuid::new_v4(),
        &[("cookie", &fx.cookie)],
        json!({ "days": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (mine, _) = legacy_key(&fx, None).await;
    for body in [
        json!({}),
        json!({ "days": 0 }),
        json!({ "days": 30, "expires_at": null }),
        json!({ "expires_at": "2000-01-01T00:00:00Z" }),
    ] {
        let (status, _) = extend_as(&fx, mine, &[("cookie", &fx.cookie)], body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}
