//! An org's block removes that org's data and nothing else (design §4.5, §5).
//!
//! An org blocks a token by revoking its grant in the token inventory, or by a
//! token policy the token breaks. An **all-access** token has no grant to be
//! confined to, so it keeps every flat route — the ones that answer across the
//! owner's orgs (chat, work, notifications, invitations) leave the blocking org
//! out, and the ones that name an org (a `workspace_id` or `org_id` query, an
//! id in the path) answer 404 for it.
//!
//! The owner here is an Owner of two orgs, A (the fixture's) and B, each seeded
//! with one of everything a flat route answers with. Org A blocks the token.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::org_invitations::{self, InviteStatus};
use entity::org_members::OrgRole;
use entity::{api_token_grants, notifications};
use oxy_authz::RoleCeiling;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::service_accounts::{in_session, with_token};
use super::stack::{Reach, get_as, ids, join_org, mint, org_grant};
use super::{Fixture, endpoint_key, fixture, legacy_key, seed_org, seed_workspace_in};

/// One of each thing a flat route answers with, in one org.
struct Seeded {
    channel: Uuid,
    work: Uuid,
    notification: Uuid,
    /// A live invitation into the org, addressed to the owner.
    invitation: String,
}

struct World {
    fx: Fixture,
    org_b: Uuid,
    ws_b: Uuid,
    a: Seeded,
    b: Seeded,
}

fn id_of(value: &Value) -> Uuid {
    value["id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(|| panic!("an id in {value}"))
}

async fn seed(fx: &Fixture, org: Uuid, label: &str) -> Seeded {
    let channel = json!({ "org_id": org, "name": format!("ops-{label}") });
    let (status, channel) = in_session(&fx.cookie, "POST", "/chat/channels", Some(channel)).await;
    assert_eq!(status, StatusCode::CREATED, "seed a channel: {channel}");

    let work = json!({ "org_id": org, "title": "Close out", "assignee_user_id": fx.user.id });
    let (status, work) = in_session(&fx.cookie, "POST", "/work", Some(work)).await;
    assert_eq!(status, StatusCode::CREATED, "seed a work item: {work}");

    let notification = Uuid::new_v4();
    notifications::ActiveModel {
        id: ActiveValue::Set(notification),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(fx.user.id),
        kind: ActiveValue::Set("work_assigned".into()),
        title: ActiveValue::Set(format!("Assigned in {label}")),
        body: ActiveValue::Set(None),
        link: ActiveValue::Set(None),
        subject_kind: ActiveValue::Set(None),
        subject_id: ActiveValue::Set(None),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        read_at: ActiveValue::Set(None),
    }
    .insert(&fx.db)
    .await
    .expect("seed a notification");

    let invitation = format!("invite-{}", Uuid::new_v4().simple());
    org_invitations::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        email: ActiveValue::Set(fx.user.email.clone().expect("an address").to_lowercase()),
        role: ActiveValue::Set(OrgRole::Member),
        invited_by: ActiveValue::Set(fx.user.id),
        token: ActiveValue::Set(invitation.clone()),
        status: ActiveValue::Set(InviteStatus::Pending),
        expires_at: ActiveValue::Set((Utc::now() + Duration::days(7)).fixed_offset()),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
    }
    .insert(&fx.db)
    .await
    .expect("seed an invitation");

    Seeded {
        channel: id_of(&channel),
        work: id_of(&work),
        notification,
        invitation,
    }
}

async fn world() -> World {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let org_b = seed_org(&fx.db).await;
    join_org(&fx.db, org_b, fx.user.id, OrgRole::Owner).await;
    let ws_b = seed_workspace_in(&fx.db, org_b).await;
    let a = seed(&fx, fx.org_id, "a").await;
    let b = seed(&fx, org_b, "b").await;
    World {
        fx,
        org_b,
        ws_b,
        a,
        b,
    }
}

/// Org A ends the token's reach from its inventory.
async fn revoke_grant(w: &World, token: Uuid) {
    let uri = format!("/orgs/{}/tokens/{token}/revoke-grant", w.fx.org_id);
    let (status, body) = in_session(&w.fx.cookie, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "revoke-grant: {body}");
    oxy_auth::token::cache::clear();
}

/// Org A refuses all-access tokens.
async fn refuse_all_access(w: &World) {
    let policy = json!({
        "max_lifetime_days": null,
        "allow_all_access_tokens": false,
        "require_environment_on_trust_policies": true,
    });
    let uri = format!("/orgs/{}/token-policy", w.fx.org_id);
    let (status, body) = in_session(&w.fx.cookie, "PUT", &uri, Some(policy)).await;
    assert_eq!(status, StatusCode::OK, "put policy: {body}");
    oxy_auth::token::cache::clear();
}

/// The ids a list route answers with, in whatever envelope it uses.
async fn listed(secret: &str, uri: &str) -> Vec<Uuid> {
    let (status, body) = get_as(secret, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    ids(body.get("items").unwrap_or(&body))
}

async fn status(secret: &str, method: &str, uri: &str, body: Option<Value>) -> StatusCode {
    with_token(secret, method, uri, body).await.0
}

/// What the token gets on every flat family once org A has blocked it.
async fn assert_only_org_b(w: &World, secret: &str, why: &str) {
    let (a, b) = (&w.a, &w.b);

    // Lists across the owner's orgs leave org A out.
    assert_eq!(listed(secret, "/chat/channels").await, [b.channel], "{why}");
    assert_eq!(listed(secret, "/work").await, [b.work], "{why}");
    assert_eq!(
        listed(secret, "/notifications").await,
        [b.notification],
        "{why}"
    );
    let (_, inbox) = get_as(secret, "/notifications").await;
    assert_eq!(inbox["unread"], 1, "org A's unread is not counted: {why}");
    let (_, invitations) = get_as(secret, "/invitations/mine").await;
    let tokens: Vec<&str> = invitations
        .as_array()
        .expect("invitations")
        .iter()
        .map(|i| i["token"].as_str().expect("a token"))
        .collect();
    assert_eq!(tokens, [b.invitation.as_str()], "{why}");

    // Asking for org A by name gets nothing of it either.
    let scoped = format!("/notifications?org_id={}", w.fx.org_id);
    assert!(listed(secret, &scoped).await.is_empty(), "{why}");

    // `read-all` clears org B's unread and leaves org A's alone.
    assert_eq!(
        status(secret, "POST", "/notifications/read-all", None).await,
        StatusCode::NO_CONTENT
    );
    let unread = notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(w.fx.user.id))
        .filter(notifications::Column::ReadAt.is_null())
        .all(&w.fx.db)
        .await
        .expect("unread notifications");
    let unread: Vec<Uuid> = unread.into_iter().map(|n| n.id).collect();
    assert_eq!(unread, [a.notification], "org A's is still unread: {why}");

    // Anything addressed to org A is not there; the same thing in org B is.
    let message = || Some(json!({ "body": "hello" }));
    for (seeded, org, reached) in [(a, w.fx.org_id, false), (b, w.org_b, true)] {
        let channel = seeded.channel;
        let probes = [
            ("GET", format!("/chat/channels/{channel}/messages"), None),
            (
                "POST",
                format!("/chat/channels/{channel}/messages"),
                message(),
            ),
            ("POST", format!("/chat/channels/{channel}/read"), None),
            ("POST", format!("/chat/channels/{channel}/join"), None),
            (
                "POST",
                "/chat/channels".to_string(),
                Some(json!({ "org_id": org, "name": "standup" })),
            ),
            (
                "POST",
                "/work".to_string(),
                Some(json!({ "org_id": org, "title": "More", "assignee_user_id": w.fx.user.id })),
            ),
            (
                "PATCH",
                format!("/work/{}", seeded.work),
                Some(json!({ "status": "in_progress" })),
            ),
            (
                "POST",
                format!("/notifications/{}/read", seeded.notification),
                None,
            ),
        ];
        for (method, uri, body) in probes {
            let got = status(secret, method, &uri, body).await;
            assert_eq!(
                got == StatusCode::NOT_FOUND,
                !reached,
                "{method} {uri} answered {got} — {why}"
            );
            if reached {
                assert!(got.is_success(), "{method} {uri} answered {got} — {why}");
            }
        }
    }

    // The routes that take the org as a `workspace_id`: 404 for org A's
    // workspace, and for org B's whatever its owner's session gets.
    for route in ["/oltp/me/connection", "/oltp/me/erd"] {
        let in_a = format!("{route}?workspace_id={}", w.fx.workspace_id);
        assert_eq!(
            status(secret, "GET", &in_a, None).await,
            StatusCode::NOT_FOUND,
            "{in_a} — {why}"
        );
    }
    let in_b = format!("/oltp/me/connection?workspace_id={}", w.ws_b);
    assert_reached(w, secret, "GET", &in_b, why).await;
    let revoke = |ws: Uuid| format!("/airhouse/me/tokens/eph_ab12?workspace_id={ws}");
    assert_eq!(
        status(secret, "DELETE", &revoke(w.fx.workspace_id), None).await,
        StatusCode::NOT_FOUND,
        "{why}"
    );
    assert_reached(w, secret, "DELETE", &revoke(w.ws_b), why).await;

    // An invitation into org A cannot be accepted with it.
    let accept = format!("/invitations/{}/accept", a.invitation);
    assert_eq!(
        status(secret, "POST", &accept, None).await,
        StatusCode::NOT_FOUND,
        "{why}"
    );

    // A GitHub flow that names org A is refused; the user's own account is not.
    for route in [
        "/user/github/account/oauth-url",
        "/user/github/installations/new-url",
    ] {
        let in_a = format!("{route}?origin=http://localhost&org_id={}", w.fx.org_id);
        assert_eq!(
            status(secret, "GET", &in_a, None).await,
            StatusCode::NOT_FOUND,
            "{in_a} — {why}"
        );
        let in_b = format!("{route}?origin=http://localhost&org_id={}", w.org_b);
        assert_reached(w, secret, "GET", &in_b, why).await;
    }
    assert_reached(w, secret, "GET", "/user/github/account", why).await;

    // Routes with no org's data in them answer as they do for a session.
    assert_eq!(
        status(secret, "GET", "/notifications/vapid-public-key", None).await,
        in_session(&w.fx.cookie, "GET", "/notifications/vapid-public-key", None)
            .await
            .0,
        "{why}"
    );
    let device = json!({ "platform": "web", "token": "https://push.example/abc" });
    let registered = status(
        secret,
        "POST",
        "/notifications/devices",
        Some(device.clone()),
    )
    .await;
    assert_ne!(registered, StatusCode::NOT_FOUND, "{why}");
    let in_a_session = in_session(&w.fx.cookie, "POST", "/notifications/devices", Some(device))
        .await
        .0;
    assert_eq!(registered, in_a_session, "registering a device: {why}");
}

/// The credential is let through to an org it reaches. What a route answers
/// past its gate depends on what this stack has — no OLTP tables, no Airhouse
/// broker, no GitHub secrets — so the assertion is the one that holds anywhere:
/// never the 404 that means "outside your reach", and exactly what the owner's
/// own session gets.
async fn assert_reached(w: &World, secret: &str, method: &str, uri: &str, why: &str) {
    let got = status(secret, method, uri, None).await;
    assert_ne!(got, StatusCode::NOT_FOUND, "{method} {uri} — {why}");
    let in_a_session = in_session(&w.fx.cookie, method, uri, None).await.0;
    assert_eq!(got, in_a_session, "{method} {uri} — {why}");
}

/// Every flat list answers with both orgs' rows.
async fn assert_both_orgs(w: &World, secret: &str, why: &str) {
    let both = |x: Uuid, y: Uuid| {
        let mut ids = vec![x, y];
        ids.sort();
        ids
    };
    let sorted = |mut ids: Vec<Uuid>| {
        ids.sort();
        ids
    };
    assert_eq!(
        sorted(listed(secret, "/chat/channels").await),
        both(w.a.channel, w.b.channel),
        "{why}"
    );
    assert_eq!(
        sorted(listed(secret, "/work").await),
        both(w.a.work, w.b.work),
        "{why}"
    );
    assert_eq!(
        sorted(listed(secret, "/notifications").await),
        both(w.a.notification, w.b.notification),
        "{why}"
    );
    let (_, invitations) = get_as(secret, "/invitations/mine").await;
    assert_eq!(
        invitations.as_array().expect("invitations").len(),
        2,
        "{why}"
    );
    let in_a = format!("/oltp/me/connection?workspace_id={}", w.fx.workspace_id);
    assert_reached(w, secret, "GET", &in_a, why).await;
    let messages = format!("/chat/channels/{}/messages", w.a.channel);
    assert_eq!(
        status(secret, "GET", &messages, None).await,
        StatusCode::OK,
        "{why}"
    );
}

#[tokio::test]
async fn an_orgs_revoke_grant_costs_an_all_access_token_that_orgs_data_only() {
    let w = world().await;
    let (id, secret) = mint(&w.fx.db, w.fx.user.id, Reach::all_access()).await;
    assert_both_orgs(&w, &secret, "before the block").await;

    revoke_grant(&w, id).await;
    assert_only_org_b(&w, &secret, "after org A's revoke-grant").await;
}

#[tokio::test]
async fn an_orgs_policy_costs_an_all_access_token_that_orgs_data_only() {
    let w = world().await;
    let (_, secret) = mint(&w.fx.db, w.fx.user.id, Reach::all_access()).await;
    assert_both_orgs(&w, &secret, "before the policy").await;

    refuse_all_access(&w).await;
    assert_only_org_b(&w, &secret, "under org A's policy").await;
}

#[tokio::test]
async fn a_grant_bound_token_is_refused_on_the_flat_routes_blocked_or_not() {
    // Unchanged by any of this: a token confined to its grants gets 404 on a
    // route that answers from raw membership, with or without a block.
    let w = world().await;
    let grants = vec![
        org_grant(w.fx.org_id, RoleCeiling::Owner),
        org_grant(w.org_b, RoleCeiling::Owner),
    ];
    let (id, secret) = mint(&w.fx.db, w.fx.user.id, Reach::granted(grants)).await;
    let flat = [
        "/chat/channels",
        "/work",
        "/notifications",
        "/invitations/mine",
        "/user/github/account",
    ];
    for blocked in [false, true] {
        if blocked {
            revoke_grant(&w, id).await;
        }
        for uri in flat {
            assert_eq!(
                status(&secret, "GET", uri, None).await,
                StatusCode::NOT_FOUND,
                "{uri} (blocked: {blocked})"
            );
        }
        let in_b = format!("/oltp/me/connection?workspace_id={}", w.ws_b);
        assert_eq!(
            status(&secret, "GET", &in_b, None).await,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn a_legacy_key_reaches_every_flat_route_whatever_an_org_decides() {
    // The hard constraint (§3.5): no policy and no stray block row narrows,
    // filters or refuses a legacy key, in either of its shapes.
    let w = world().await;
    let (seeded_id, seeded) = legacy_key(&w.fx, None).await;
    let (minted_id, minted) = endpoint_key(&w.fx, None).await;
    assert_both_orgs(&w, &seeded, "a seeded legacy key").await;
    assert_both_orgs(&w, &minted, "a key the legacy endpoint minted").await;

    // The tightest policy an org can set…
    let policy = json!({
        "max_lifetime_days": 1,
        "allow_all_access_tokens": false,
        "require_environment_on_trust_policies": true,
    });
    for org in [w.fx.org_id, w.org_b] {
        let uri = format!("/orgs/{org}/token-policy");
        let (status, body) = in_session(&w.fx.cookie, "PUT", &uri, Some(policy.clone())).await;
        assert_eq!(status, StatusCode::OK, "put policy: {body}");
    }
    // …revoke-grant, which refuses a legacy key outright…
    for key in [seeded_id, minted_id] {
        let token = super::token_row(&w.fx.db, key)
            .await
            .expect("the key's mirror row");
        let uri = format!("/orgs/{}/tokens/{}/revoke-grant", w.fx.org_id, token.id);
        let (status, _) = in_session(&w.fx.cookie, "POST", &uri, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "a legacy key's grant");
        assert!(
            api_token_grants::Entity::find()
                .filter(api_token_grants::Column::TokenId.eq(token.id))
                .all(&w.fx.db)
                .await
                .unwrap()
                .is_empty(),
            "nothing was written beside a legacy key"
        );
    }
    oxy_auth::token::cache::clear();

    // …and both keys still answer with both orgs, on every family.
    assert_both_orgs(&w, &seeded, "a seeded legacy key, after").await;
    assert_both_orgs(&w, &minted, "an endpoint-minted key, after").await;
}
