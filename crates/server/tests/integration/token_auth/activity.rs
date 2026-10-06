//! `GET /api/{workspace_id}/api-keys/{id}/activity` (design §3.7; contract in
//! the Phase 1 PR): the key's lifecycle events and the actions performed with
//! it, its daily usage, and where it was last used — for its owner, in a
//! browser session only.

use super::*;
use axum::http::StatusCode;
use oxy_auth::token::usage;

async fn activity(
    fx: &Fixture,
    id: Uuid,
    query: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let uri = format!("/{}/api-keys/{id}/activity{query}", fx.workspace_id);
    call(api_surface(), "GET", &uri, headers, None).await
}

async fn get_as(fx: &Fixture, router: Router, path: &str, headers: &[(&str, &str)]) -> StatusCode {
    let uri = format!("/{}/{path}", fx.workspace_id);
    call(router, "GET", &uri, headers, None).await.0
}

#[tokio::test]
async fn activity_returns_lifecycle_and_action_events_and_usage_after_a_flush() {
    let fx = fixture().await;
    let (id, key) = legacy_key(&fx, Some(Utc::now() + chrono::Duration::days(1))).await;
    let keyed = [
        ("x-api-key", key.as_str()),
        ("x-forwarded-for", "203.0.113.4"),
        ("user-agent", "oxyc/0.5.0"),
    ];
    let session = [("cookie", fx.cookie.as_str())];

    // A lifecycle event (session), then an action and some traffic (key).
    let uri = format!("/{}/api-keys/{id}/extend", fx.workspace_id);
    let (status, _) = call(
        api_surface(),
        "POST",
        &uri,
        &session,
        Some(json!({ "expires_at": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        get_as(&fx, audited_surface(&fx), "audited", &keyed).await,
        StatusCode::OK
    );
    assert_eq!(
        get_as(&fx, api_surface(), "probe", &keyed).await,
        StatusCode::OK
    );
    assert_eq!(
        get_as(&fx, api_surface(), "teapot", &keyed).await.as_u16(),
        418
    );

    // Before the flush: events are there, usage is not, and last_used falls
    // back to the token's own last_used_at (when, but not from where).
    let (status, body) = activity(&fx, id, "", &session).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["usage"], json!([]));
    assert!(body["last_used"]["at"].is_string(), "{body}");
    assert_eq!(body["last_used"]["route"], Value::Null);

    usage::flush(&fx.db).await.expect("flush");
    let (status, body) = activity(&fx, id, "", &session).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Newest first: the action performed with the key, then its lifecycle event.
    let events = body["events"].as_array().expect("events");
    let actions: Vec<&str> = events
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, [AUDITED_ACTION, "token.extended"]);
    let action = &events[0];
    assert_eq!(action["actor_type"], "api_key");
    assert_eq!(action["metadata"], json!({ "token_id": id }));
    let extended = &events[1];
    assert_eq!(extended["actor_type"], "user");
    assert_eq!(extended["target_type"], "api_token");
    assert_eq!(extended["target_id"], json!(id.to_string()));
    assert_eq!(extended["metadata"]["token_id"], json!(id));
    assert!(extended["metadata"]["old_expires_at"].is_string());
    assert_eq!(extended["metadata"]["new_expires_at"], Value::Null);
    assert!(
        extended["metadata"]
            .as_object()
            .unwrap()
            .contains_key("new_expires_at"),
        "null means no expiry, and the key is present"
    );
    // The admin row shape, and nothing the contract does not name in metadata.
    for field in [
        "id",
        "created_at",
        "actor_email",
        "outcome",
        "via_global_override",
    ] {
        assert!(action.get(field).is_some(), "{field}");
    }
    assert!(action["metadata"].get("token_name").is_none());

    // Usage: one day, three keyed requests (audited, probe, teapot), one 4xx.
    let usage_days = body["usage"].as_array().expect("usage");
    assert_eq!(usage_days.len(), 1);
    assert_eq!(
        usage_days[0]["day"],
        json!(Utc::now().date_naive().to_string())
    );
    assert_eq!(usage_days[0]["requests"], 3);
    assert_eq!(usage_days[0]["errors_4xx"], 1);
    assert_eq!(usage_days[0]["errors_5xx"], 0);

    let last = &body["last_used"];
    assert_eq!(last["ip"], "203.0.113.4");
    assert_eq!(last["user_agent"], "oxyc/0.5.0");
    assert_eq!(
        last["route"], "/{workspace_id}/teapot",
        "the template, not the path"
    );
    assert!(last["at"].is_string());

    // Nothing in the response is the key.
    assert!(!body.to_string().contains(&key));

    let (_, one) = activity(&fx, id, "?limit=1", &session).await;
    assert_eq!(one["events"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_key_never_used_has_no_last_used() {
    let fx = fixture().await;
    let (id, _) = legacy_key(&fx, None).await;
    let (status, body) = activity(&fx, id, "", &[("cookie", &fx.cookie)]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "events": [], "usage": [], "last_used": null })
    );
}

#[tokio::test]
async fn activity_is_404_for_someone_elses_key_and_an_unknown_one() {
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
    for id in [theirs, Uuid::new_v4()] {
        let (status, body) = activity(&fx, id, "", &[("cookie", &fx.cookie)]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(body.get("events").is_none());
    }
}

#[tokio::test]
async fn activity_refuses_a_token_authenticated_caller() {
    let fx = fixture().await;
    let (legacy_id, legacy) = legacy_key(&fx, None).await;
    let (pat_id, pat) = minted_pat(&fx, None).await;
    let bearer = format!("Bearer {pat}");
    for (id, header) in [
        (legacy_id, ("x-api-key", legacy.as_str())),
        (pat_id, ("authorization", bearer.as_str())),
    ] {
        let (status, body) = activity(&fx, id, "", &[header]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{header:?}");
        assert_eq!(
            body,
            json!({ "error": "activity requires a browser session" })
        );
    }
}
