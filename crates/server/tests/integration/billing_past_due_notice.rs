//! Billing's past-due notice against a real Oxy database: every owner is told,
//! once per episode and per org, with no org OLTP store, and the notice is
//! critical, so no mute silences it.
//!
//! Moved out of `oxy-api-notifications`' `platform_notices` with the
//! dispatcher (decompose B2): `past_due_notice` is `oxy-api-billing`'s now,
//! and the read-back (delivery, mutes, the inbox) is the notifications
//! surface's, and a surface may not depend on another, not even for its
//! tests. The seed helpers are that group's, included by path.
//!
//! Run with:
//! `cargo nextest run -p oxy-server --test integration -E 'test(billing_past_due_notice)'`

use chrono::{TimeZone, Utc};
use entity::notification_sends;
use entity::org_members::OrgRole;
use entity::workspaces::WorkspaceStatus;
use oxy_api_billing::billing::notify::past_due_notice;
use oxy_api_notifications::delivery::{Channel, deliver_send};
use oxy_api_notifications::inbox_view;
use oxy_api_notifications::platform::kinds::BILLING_PAST_DUE;
use oxy_api_notifications::platform::preferences;
use oxy_api_notifications::settings::model::Node;
use oxy_api_notifications::settings::store;
use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use crate::notices_fixture::{join, people, push_outcome, seed_org, seed_workspace, sorted, told};

/// Billing tells every owner, once per episode, needs no OLTP store, and is
/// critical: a mute row cannot silence it, and the settings route refuses to
/// write one.
#[tokio::test]
async fn platform_notices_billing_reaches_every_owner_once_and_cannot_be_muted() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    seed_workspace(&db, org.id, "Default", WorkspaceStatus::Ready, None).await;
    let grace = Utc.with_ymd_and_hms(2026, 10, 21, 9, 0, 0).unwrap();

    let sent = past_due_notice(&db, org.id, grace).await.expect("notice");
    assert_eq!(sent.recipients, 2);
    let again = past_due_notice(&db, org.id, grace).await.expect("replay");
    assert_eq!(
        (again.recipients, again.already_told),
        (0, 2),
        "a replayed webhook tells nobody twice"
    );

    let rows = told(&db, BILLING_PAST_DUE, "2026-10-21T09:00:00Z").await;
    assert_eq!(people(&rows), sorted(vec![org.owner, org.second_owner]));
    let send = notification_sends::Entity::find_by_id(sent.id.unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        send.critical && !send.email,
        "critical, and the owner email is billing's own"
    );
    assert!(
        send.run_id.is_some(),
        "queued under the org's workspace, not delivered inline"
    );

    // The settings route will not write the mute; a row written anyway is
    // ignored at delivery.
    assert_eq!(
        preferences::set(&db, org.owner, BILLING_PAST_DUE, Channel::Push, true).await,
        Err(axum::http::StatusCode::BAD_REQUEST)
    );
    let billing = Node::Kind(BILLING_PAST_DUE.into());
    store::set_channel(&db, org.owner, None, &billing, Channel::Push, Some(false))
        .await
        .expect("seed a mute");
    deliver_send(&db, sent.id.unwrap()).await;
    let owners_signal = rows.iter().find(|r| r.user_id == org.owner).unwrap();
    assert_ne!(
        push_outcome(&db, owners_signal.id).await.1.as_deref(),
        Some("muted"),
        "a critical kind ignores mutes"
    );

    let items = inbox_view::items(&db, vec![owners_signal.clone()])
        .await
        .expect("inbox");
    assert_eq!(items[0].title.as_deref(), Some("Payment past due for Acme"));
    assert!(
        items[0]
            .body
            .as_deref()
            .unwrap()
            .contains("October 21, 2026")
    );
    assert_eq!(
        items[0].href.as_deref(),
        Some(format!("/{}?settings=organization.billing", org.slug).as_str())
    );
    assert!(items[0].app.is_none() && items[0].link.is_none());
}

/// Billing's subject is the grace period's end, which two orgs can share to the
/// second. A person who owns both must hear about each: "already told" is per
/// org, not per subject alone.
#[tokio::test]
async fn platform_notices_billing_tells_an_owner_of_two_orgs_about_each() {
    let (db, _url) = fresh_db(Schema::All).await;
    let first = seed_org(&db).await;
    let second = seed_org(&db).await;
    join(&db, second.id, first.owner, OrgRole::Owner).await;
    seed_workspace(&db, first.id, "Default", WorkspaceStatus::Ready, None).await;
    seed_workspace(&db, second.id, "Default", WorkspaceStatus::Ready, None).await;
    let grace = Utc.with_ymd_and_hms(2026, 10, 21, 9, 0, 0).unwrap();

    past_due_notice(&db, first.id, grace)
        .await
        .expect("first org");
    let sent = past_due_notice(&db, second.id, grace)
        .await
        .expect("second org");
    assert_eq!(
        (sent.recipients, sent.already_told),
        (3, 0),
        "the shared owner is told about the second org too"
    );

    let shared_owner_orgs: Vec<Uuid> = told(&db, BILLING_PAST_DUE, "2026-10-21T09:00:00Z")
        .await
        .into_iter()
        .filter(|r| r.user_id == first.owner)
        .map(|r| r.org_id)
        .collect();
    assert_eq!(sorted(shared_owner_orgs), sorted(vec![first.id, second.id]));
}
