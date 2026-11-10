//! `GET`/`PATCH /api/notifications/settings` against a real database: nodes
//! written and decided nearest-first, `If-Match` refusing a stale version, a
//! refused patch writing nothing, the old preferences and catalogue routes
//! reading and writing the same table, and the migration's backfill of the
//! three old mute tables.

use entity::{notification_mutes, notification_platform_mutes, notification_scope_mutes};
use migration::NOTIFICATION_SETTINGS_BACKFILL_SQL;
use oxy_api_notifications::catalog::handler::catalog_for;
use oxy_api_notifications::delivery::Channel;
use oxy_api_notifications::preferences::preferences_for;
use oxy_api_notifications::scope_mutes;
use oxy_api_notifications::settings::document::{NotificationSettings, document_for};
use oxy_api_notifications::settings::model::Node;
use oxy_api_notifications::settings::patch::{PatchError, apply};
use oxy_api_notifications::settings::reconcile::{Reconciled, reconcile};
use oxy_api_notifications::settings::store;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    EntityTrait, PaginatorTrait, QueryFilter,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{World, seed};
use crate::common::Schema;
use crate::oltp_fx::with_fx_on;

async fn patch(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    if_match: Option<&str>,
    body: Value,
) -> Result<NotificationSettings, PatchError> {
    apply(db, user, if_match, body.to_string().as_bytes()).await
}

fn refusal(r: Result<NotificationSettings, PatchError>) -> &'static str {
    match r {
        Err(PatchError::BadRequest(code)) => code,
        Err(e) => panic!("expected a 400, got {e:?}"),
        Ok(_) => panic!("expected a 400, it applied"),
    }
}

fn store_ops(doc: &NotificationSettings) -> Value {
    let v = serde_json::to_value(doc).unwrap();
    v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["app"]["slug"] == "store-ops")
        .cloned()
        .expect("the app is listed")
}

/// The node at `path` (a category path, then optionally a kind) of `source`.
fn at(source: &Value, path: &[&str], kind: Option<&str>) -> Value {
    let mut node = source.clone();
    for p in path {
        node = node["categories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == *p)
            .cloned()
            .unwrap_or_else(|| panic!("no category {p}"));
    }
    match kind {
        None => node,
        Some(kind) => node["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == kind)
            .cloned()
            .unwrap_or_else(|| panic!("no kind {kind}")),
    }
}

fn app_node(w: &World, mut node: Value) -> Value {
    node["app_id"] = json!(w.app.id);
    node
}

#[tokio::test]
async fn notification_categories_settings_nearest_explicit_wins_and_old_routes_agree() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let (db, me) = (&fx.db, &w.member);
        let empty = document_for(db, me).await.expect("document");
        assert_eq!(empty.version, "0");
        let v = serde_json::to_value(&empty).unwrap();
        assert_eq!(
            v["global"],
            json!({ "paused_until": null, "quiet_hours": null, "timezone": null })
        );
        assert_eq!(v["devices"], json!([]));
        assert_eq!(
            v["reach"],
            json!({ "push_devices": 0, "email": true, "slack": false })
        );

        let doc = patch(
            db,
            me,
            Some("0"),
            json!({ "nodes": [
            app_node(&w, json!({ "scope": "", "channels": { "push": false } })),
            app_node(&w, json!({ "scope": "operations", "channels": { "push": true } })),
            app_node(&w, json!({ "scope": "operations/receiving", "min_level": "time_sensitive" })),
            app_node(&w, json!({ "kind": "list_overdue", "channels": { "email": false } })),
        ] }),
        )
        .await
        .expect("applied");
        assert_eq!(doc.version, "1");
        let app = store_ops(&doc);
        assert_eq!(
            app["settings"],
            json!({ "channels": { "push": false }, "min_level": null })
        );
        let ops = at(&app, &["operations"], None);
        assert_eq!(ops["settings"]["channels"], json!({ "push": true }));
        let effective = |path: &[&str], kind| at(&app, path, Some(kind))["effective"].clone();
        assert_eq!(
            effective(&["operations"], "list_overdue")["push"],
            json!({ "on": true, "reason": null, "from": "Store operations" }),
            "a category turns push back on under a source that is off"
        );
        assert_eq!(
            effective(&["operations"], "list_overdue")["email"],
            json!({ "on": false, "reason": "muted", "from": "Overdue lists" })
        );
        assert_eq!(
            effective(&["general"], "shift_note")["push"],
            json!({ "on": false, "reason": "muted", "from": "App store-ops" })
        );
        for channel in ["push", "email", "slack"] {
            assert_eq!(
                effective(&["operations", "operations/receiving"], "delivery_late")[channel],
                json!({ "on": false, "reason": "below_level", "from": "Receiving" }),
                "{channel}"
            );
        }
        assert_eq!(
            effective(&["operations", "operations/food-safety"], "temp_alarm")["push"],
            json!({ "on": true, "reason": null, "from": null }),
            "critical ignores the source being off"
        );

        // Stale: refused with the current document, nothing written.
        let stale = patch(
            db,
            me,
            Some("\"0\""),
            json!({ "global": { "timezone": "UTC" } }),
        )
        .await;
        let Err(PatchError::Stale(current)) = stale else {
            panic!("a stale If-Match must be refused");
        };
        assert_eq!(
            (current.version.as_str(), current.global.timezone.as_deref()),
            ("1", None)
        );

        // The catalogue's `muted` is the `false` entries; an explicit on is
        // not one.
        let catalog = serde_json::to_value(catalog_for(db, me).await.unwrap()).unwrap();
        let listed = catalog["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["app"]["slug"] == "store-ops")
            .cloned()
            .unwrap();
        assert_eq!(listed["muted"], json!(["push"]));
        assert_eq!(at(&listed, &["operations"], None)["muted"], json!([]));
        assert_eq!(
            at(&listed, &["operations"], Some("list_overdue"))["muted"],
            json!(["email"])
        );
        let prefs = serde_json::to_value(preferences_for(db, me).await.unwrap()).unwrap();
        let kinds = &prefs["apps"][0]["kinds"];
        let overdue = kinds
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == "list_overdue");
        assert_eq!(overdue.unwrap()["muted"], json!(["email"]));

        // The old route writes the same table and bumps the version.
        let old = scope_mutes::set(db, me, Some(w.app.id), "operations", Channel::Slack, true);
        old.await.expect("old route");
        let doc = document_for(db, me).await.unwrap();
        assert_eq!(doc.version, "2");
        let ops = at(&store_ops(&doc), &["operations"], None);
        assert_eq!(
            ops["settings"]["channels"],
            json!({ "push": true, "slack": false })
        );

        // Mutes are mirrored into the old tables for a rollback, and cleared
        // with them.
        let mirrored = || {
            notification_scope_mutes::Entity::find()
                .filter(notification_scope_mutes::Column::UserId.eq(me.id))
                .filter(notification_scope_mutes::Column::Scope.eq(""))
                .count(db)
        };
        assert_eq!(mirrored().await.unwrap(), 1);
        patch(
            db,
            me,
            None,
            json!({ "nodes": [
            app_node(&w, json!({ "scope": "", "channels": { "push": null } })),
        ] }),
        )
        .await
        .expect("back to inherit");
        assert_eq!(mirrored().await.unwrap(), 0);
        let source = store::node(db, me.id, Some(w.app.id), &Node::source())
            .await
            .unwrap();
        assert!(source.is_empty(), "a node with nothing set has no row");
    })
    .await;
}

#[tokio::test]
async fn notification_categories_settings_a_refused_patch_writes_nothing() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let (db, me) = (&fx.db, &w.member);
        let node = |v| json!({ "nodes": [app_node(&w, v)] });
        assert_eq!(
            refusal(patch(db, &w.stranger, None, node(json!({ "scope": "", "channels": { "push": false } }))).await),
            "unknown_node",
            "an app that cannot tell this person anything"
        );
        // One transaction: the first node is valid, the second is not.
        let two = json!({ "nodes": [
            app_node(&w, json!({ "scope": "operations", "channels": { "push": false } })),
            app_node(&w, json!({ "scope": "finance", "channels": { "push": false } })),
        ] });
        assert_eq!(refusal(patch(db, me, None, two).await), "unknown_node");
        let operations = Node::Scope("operations".into());
        let ops = store::node(db, me.id, Some(w.app.id), &operations).await.unwrap();
        assert!(ops.is_empty(), "nothing of a refused patch is written");
        assert_eq!(
            refusal(patch(db, me, None, node(json!({ "scope": "operations/food-safety",
                                                    "channels": { "push": false } }))).await),
            "critical_only"
        );
        let oxygen = json!({ "nodes": [{ "app_id": null, "kind": "oxy.workspace_ready",
                                         "channels": { "email": false } }] });
        assert_eq!(refusal(patch(db, me, None, oxygen).await), "channel_not_used");
        let quiet = json!({ "start": "22:00", "end": "07:00", "days": [1, 2], "allow": "time_sensitive" });
        assert_eq!(
            refusal(patch(db, me, None, json!({ "global": { "quiet_hours": quiet } })).await),
            "timezone_required"
        );
        let mut late = quiet.clone();
        late["start"] = json!("25:00");
        assert_eq!(
            refusal(patch(db, me, None, json!({ "global": { "quiet_hours": late, "timezone": "UTC" } })).await),
            "bad_time"
        );
        assert_eq!(refusal(apply(db, me, None, b"{\"nodes\": 1}").await), "invalid_body");
        assert_eq!(document_for(db, me).await.unwrap().version, "0", "no refusal bumps it");

        let doc = patch(db, me, Some("0"), json!({ "global": {
            "quiet_hours": quiet, "timezone": "Europe/Stockholm", "paused_until": "2030-01-01T00:00:00Z" } }))
        .await
        .expect("applied");
        assert_eq!(
            serde_json::to_value(&doc.global).unwrap(),
            json!({ "paused_until": "2030-01-01T00:00:00Z", "timezone": "Europe/Stockholm",
                    "quiet_hours": { "start": "22:00", "end": "07:00", "days": [1, 2],
                                     "allow": "time_sensitive" } })
        );
    })
    .await;
}

#[tokio::test]
async fn notification_categories_settings_backfill_copies_the_three_mute_tables() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let db = &fx.db;
        let me = w.member.id;
        let now = chrono::Utc::now().fixed_offset();
        for channel in ["push", "email"] {
            notification_mutes::ActiveModel {
                user_id: Set(me),
                app_id: Set(w.app.id),
                kind: Set("list_overdue".into()),
                channel: Set(channel.into()),
                created_at: Set(now),
            }
            .insert(db)
            .await
            .expect("kind mute");
        }
        notification_platform_mutes::ActiveModel {
            user_id: Set(me),
            kind: Set("oxy.run_failed".into()),
            channel: Set("slack".into()),
            created_at: Set(now),
        }
        .insert(db)
        .await
        .expect("platform mute");
        for (app, scope) in [(Some(w.app.id), "operations"), (None, "")] {
            notification_scope_mutes::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(me),
                app_id: Set(app),
                scope: Set(scope.into()),
                channel: Set("push".into()),
                created_at: Set(now),
            }
            .insert(db)
            .await
            .expect("scope mute");
        }
        for _ in 0..2 {
            db.execute_unprepared(NOTIFICATION_SETTINGS_BACKFILL_SQL)
                .await
                .expect("backfill, and again: it is idempotent");
        }
        let set = store::nodes_for_user(db, me).await.unwrap();
        let app = &set[&Some(w.app.id)];
        let overdue = &app[&Node::Kind("list_overdue".into())];
        assert_eq!(overdue.off(), vec!["push", "email"]);
        assert_eq!(overdue.slack, None, "an unmuted channel still inherits");
        assert_eq!(app[&Node::Scope("operations".into())].off(), vec!["push"]);
        let oxygen = &set[&None];
        assert_eq!(
            oxygen[&Node::Kind("oxy.run_failed".into())].off(),
            vec!["slack"]
        );
        assert_eq!(oxygen[&Node::source()].off(), vec!["push"]);
        assert_eq!(
            set.values().map(|n| n.len()).sum::<usize>(),
            4,
            "one row per node"
        );
        assert!(
            store::nodes_for_user(db, w.other.id)
                .await
                .unwrap()
                .is_empty(),
            "nobody else's"
        );
    })
    .await;
}

/// What a previous release writes to the old mute tables while rolled back
/// to — a mute, an unmute — reaches the settings table at the next boot,
/// and nothing else moves: an explicit on and a floor stay, and with the
/// mirror in step it changes nothing.
#[tokio::test]
async fn notification_categories_settings_reconcile_brings_back_a_rollbacks_mutes() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let (db, me) = (&fx.db, &w.member);
        patch(
            db,
            me,
            None,
            json!({ "nodes": [
            app_node(&w, json!({ "kind": "list_overdue", "channels": { "push": false } })),
            app_node(&w, json!({ "scope": "operations", "channels": { "slack": false } })),
            app_node(&w, json!({ "kind": "stock_low", "channels": { "push": true },
                                 "min_level": "critical" })),
        ] }),
        )
        .await
        .expect("applied");
        patch(
            db,
            &w.other,
            None,
            json!({ "nodes": [
            app_node(&w, json!({ "kind": "list_overdue", "channels": { "push": false } })),
        ] }),
        )
        .await
        .expect("applied");
        assert_eq!(
            reconcile(db).await.unwrap(),
            Reconciled::default(),
            "the mirror keeps the two in step: nothing to do"
        );

        // The previous release, rolled back to: writes to the old tables only.
        let now = chrono::Utc::now().fixed_offset();
        let old_mute = |kind: &str, channel: &str| notification_mutes::ActiveModel {
            user_id: Set(me.id),
            app_id: Set(w.app.id),
            kind: Set(kind.into()),
            channel: Set(channel.into()),
            created_at: Set(now),
        };
        old_mute("list_overdue", "email").insert(db).await.unwrap();
        old_mute("stock_low", "push").insert(db).await.unwrap();
        notification_mutes::Entity::delete_many()
            .filter(notification_mutes::Column::UserId.eq(me.id))
            .filter(notification_mutes::Column::Kind.eq("list_overdue"))
            .filter(notification_mutes::Column::Channel.eq("push"))
            .exec(db)
            .await
            .unwrap();
        notification_scope_mutes::Entity::delete_many()
            .filter(notification_scope_mutes::Column::UserId.eq(me.id))
            .exec(db)
            .await
            .unwrap();
        notification_platform_mutes::ActiveModel {
            user_id: Set(me.id),
            kind: Set("oxy.run_failed".into()),
            channel: Set("slack".into()),
            created_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
        let version = document_for(db, me).await.unwrap().version;

        // Two pods booting at once: one does the work, the other finds none.
        let (a, b) = tokio::join!(reconcile(db), reconcile(db));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(a.people + b.people, 1, "{a:?} {b:?}");
        assert_eq!(
            a.muted + b.muted,
            2,
            "email on list_overdue, slack on run_failed"
        );
        assert_eq!(
            a.unmuted + b.unmuted,
            2,
            "push on list_overdue, slack on operations"
        );
        assert_ne!(
            document_for(db, me).await.unwrap().version,
            version,
            "a new version"
        );

        let set = store::nodes_for_user(db, me.id).await.unwrap();
        let app = &set[&Some(w.app.id)];
        let overdue = &app[&Node::Kind("list_overdue".into())];
        assert_eq!((overdue.push, overdue.email), (None, Some(false)));
        assert!(
            !app.contains_key(&Node::Scope("operations".into())),
            "a node left with nothing set has no row"
        );
        let stock = &app[&Node::Kind("stock_low".into())];
        assert_eq!(
            stock.push,
            Some(true),
            "an explicit on is not the old binary's to change"
        );
        assert_eq!(
            stock.min_level,
            Some(oxy_api_notifications::catalog::model::Level::Critical)
        );
        assert_eq!(
            set[&None][&Node::Kind("oxy.run_failed".into())].off(),
            vec!["slack"]
        );
        let theirs = store::nodes_for_user(db, w.other.id).await.unwrap();
        assert_eq!(
            theirs[&Some(w.app.id)][&Node::Kind("list_overdue".into())].off(),
            vec!["push"],
            "nobody else's moved"
        );
        assert_eq!(
            reconcile(db).await.unwrap(),
            Reconciled::default(),
            "a repeat is a no-op"
        );
    })
    .await;
}

/// Backends of this test's database waiting on a lock.
async fn waiting_on_a_lock(db: &DatabaseConnection) -> i64 {
    let row = db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock' \
               AND pid <> pg_backend_pid()",
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "n").unwrap()
}

/// A reconcile at boot takes a person's settings lock before their nodes —
/// the order every writer takes them in — so a write in flight finishes
/// first and the two never deadlock: here the write touches the very node
/// the reconcile is about to change.
#[tokio::test]
async fn notification_categories_settings_reconcile_waits_for_a_write_in_flight() {
    with_fx_on(Schema::All, |fx| async move {
        let w = seed(&fx.db, fx.org_id).await;
        let (db, me) = (&fx.db, &w.member);
        patch(
            db,
            me,
            None,
            json!({ "nodes": [
            app_node(&w, json!({ "kind": "list_overdue", "channels": { "push": false } })),
        ] }),
        )
        .await
        .expect("applied");
        // The previous release muted email on the same kind; the settings
        // table inherits there, so the reconcile has that node to change.
        notification_mutes::ActiveModel {
            user_id: Set(me.id),
            app_id: Set(w.app.id),
            kind: Set("list_overdue".into()),
            channel: Set("email".into()),
            created_at: Set(chrono::Utc::now().fixed_offset()),
        }
        .insert(db)
        .await
        .unwrap();
        // A write of this person's is in flight: it holds their settings row.
        let in_flight = sea_orm::TransactionTrait::begin(db).await.unwrap();
        store::lock(&in_flight, me.id).await.unwrap();

        let pending = tokio::spawn({
            let db = db.clone();
            async move { reconcile(&db).await }
        });
        // The condition, not a duration: the reconcile is seen waiting.
        let mut waited = false;
        for _ in 0..200 {
            if waiting_on_a_lock(db).await > 0 {
                waited = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(waited, "the reconcile never waited for the write in flight");

        // The write goes on to the node the reconcile wants: it unmutes push.
        let overdue = Node::Kind("list_overdue".into());
        let mut next = store::node(&in_flight, me.id, Some(w.app.id), &overdue)
            .await
            .unwrap();
        next.push = None;
        store::put_node(&in_flight, me.id, Some(w.app.id), &overdue, &next)
            .await
            .expect("the write is not blocked by the reconcile, nor aborted as a deadlock");
        in_flight.commit().await.expect("the write commits");

        let done = pending
            .await
            .unwrap()
            .expect("the reconcile is not aborted either");
        assert_eq!((done.muted, done.unmuted, done.people), (1, 0, 1));
        let after = store::node(db, me.id, Some(w.app.id), &overdue)
            .await
            .unwrap();
        assert_eq!(
            (after.push, after.email),
            (None, Some(false)),
            "both stand: the unmute, and the previous release's mute"
        );
    })
    .await;
}
