//! A mute at a category covers its subtree, a mute on the source covers
//! everything, and a kind the catalogue makes critical ignores both — decided
//! by delivery against the app's live build.

use axum::http::StatusCode;
use entity::{notification_deliveries, notifications};
use oxy_api_notifications::delivery::{Channel, deliver_send};
use oxy_api_notifications::scope_mutes;
use oxy_api_notifications::send::{AppRef, Outgoing, send};
use oxy_api_notifications::settings::model::Node;
use oxy_api_notifications::settings::store;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::fixture::{World, seed, seed_app};
use crate::common::Schema;
use crate::oltp_fx::with_fx_on;

fn outgoing(org: Uuid, w: &World, kind: &str) -> Outgoing {
    Outgoing {
        org_id: org,
        app: Some(AppRef {
            id: w.app.id,
            slug: w.app.slug.clone(),
            name: w.app.name.clone(),
        }),
        environment: "production".into(),
        workspace_id: Some(w.workspace),
        kind: kind.into(),
        // What the function said. `temp_alarm` is critical by the catalogue
        // alone, and delivery must see that.
        critical: false,
        title: "Heads up".into(),
        body: None,
        link: None,
        email: None,
        slack: false,
        idempotency_key: None,
        thread_key: None,
        recipients: vec![w.member.id, w.other.id],
    }
}

/// The push outcome of a send for `user`: `muted`, or `not_configured` (no
/// transport is registered in a test process) when nothing muted it.
async fn push_reason(db: &DatabaseConnection, send_id: Uuid, user: Uuid) -> String {
    let signal = notifications::Entity::find()
        .filter(notifications::Column::MessageId.eq(send_id))
        .filter(notifications::Column::UserId.eq(user))
        .one(db)
        .await
        .expect("query")
        .expect("a signal");
    notification_deliveries::Entity::find_by_id((signal.id, "push".to_string()))
        .one(db)
        .await
        .expect("query")
        .expect("a push delivery")
        .reason
        .unwrap_or_default()
}

async fn sent(db: &DatabaseConnection, org: Uuid, w: &World, kind: &str) -> Uuid {
    let sent = send(db, outgoing(org, w, kind)).await.expect("send");
    let report = deliver_send(db, sent.id).await;
    assert_eq!(report.failed, 0, "{report:?}");
    sent.id
}

#[tokio::test]
async fn notification_categories_ancestor_mutes_reach_down_and_critical_ignores_them() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let w = seed(&fx.db, fx.org_id).await;
        let db = &fx.db;
        let set = |user, scope: &'static str| {
            let app = Some(w.app.id);
            async move { scope_mutes::set(db, user, app, scope, Channel::Push, true).await }
        };
        // The member mutes the `operations` category; the other member the
        // whole app.
        assert_eq!(
            set(&w.member, "operations").await,
            Ok(StatusCode::NO_CONTENT)
        );
        assert_eq!(set(&w.other, "").await, Ok(StatusCode::NO_CONTENT));

        // In `operations` itself.
        let id = sent(db, fx.org_id, &w, "list_overdue").await;
        assert_eq!(push_reason(db, id, w.member.id).await, "muted");
        assert_eq!(push_reason(db, id, w.other.id).await, "muted");

        // A category below it: the mute reaches down.
        let id = sent(db, fx.org_id, &w, "delivery_late").await;
        assert_eq!(push_reason(db, id, w.member.id).await, "muted");

        // `general`: outside the member's category, inside the other's source.
        let id = sent(db, fx.org_id, &w, "shift_note").await;
        assert_eq!(push_reason(db, id, w.member.id).await, "not_configured");
        assert_eq!(push_reason(db, id, w.other.id).await, "muted");

        // Critical by the catalogue: no mute at any level reaches it.
        let id = sent(db, fx.org_id, &w, "temp_alarm").await;
        assert_eq!(push_reason(db, id, w.member.id).await, "not_configured");
        assert_eq!(push_reason(db, id, w.other.id).await, "not_configured");

        // Unmuting the category lets `operations` through again.
        let unmute = scope_mutes::set(
            db,
            &w.member,
            Some(w.app.id),
            "operations",
            Channel::Push,
            false,
        );
        assert_eq!(unmute.await, Ok(StatusCode::NO_CONTENT));
        let id = sent(db, fx.org_id, &w, "stock_low").await;
        assert_eq!(push_reason(db, id, w.member.id).await, "not_configured");
    })
    .await;
}

#[tokio::test]
async fn notification_categories_a_scope_mute_is_checked_against_the_live_catalogue() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let db = &fx.db;
        let mute = |user, app, scope: &'static str| {
            scope_mutes::set(db, user, app, scope, Channel::Push, true)
        };
        assert_eq!(
            mute(&w.member, Some(w.app.id), "finance").await,
            Err(StatusCode::BAD_REQUEST),
            "a path the live build does not have"
        );
        assert_eq!(
            mute(&w.member, Some(w.app.id), "operations/food-safety").await,
            Err(StatusCode::BAD_REQUEST),
            "every kind below is critical: the mute could never apply"
        );
        assert_eq!(
            mute(&w.stranger, Some(w.app.id), "operations").await,
            Err(StatusCode::NOT_FOUND),
            "an app that cannot tell this person anything is not theirs to mute"
        );
        // Oxygen: `app_id: null`. Billing is critical-only.
        assert_eq!(
            mute(&w.member, None, "billing").await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            mute(&w.member, None, "runs").await,
            Ok(StatusCode::NO_CONTENT)
        );
        let runs = Node::Scope("runs".into());
        let set = |user| store::node(db, user, None, &runs);
        assert_eq!(set(w.member.id).await.expect("query").off(), vec!["push"]);
        assert!(
            set(w.other.id).await.expect("query").is_empty(),
            "the other member muted nothing"
        );
        // A second app's scope mute is that app's alone.
        let other_app = seed_app(db, fx.org_id, w.workspace, "other-app", None).await;
        let theirs = store::node(db, w.member.id, Some(other_app.id), &Node::source())
            .await
            .expect("query");
        assert!(theirs.is_empty());
    })
    .await;
}
