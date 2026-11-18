//! Provenance (R25(c)): the relay delivers only rows the host signed. App
//! SQL can call `oxy_events.publish` itself — a `DO` block, or a function the
//! app defined — but it never holds the org's key, so the MAC it passes does
//! not match the row's fields: such a row is stored with `relay_error`
//! `unverified_provenance`, delivered to nobody, and its body kept. So is a
//! row the host signed whose fields changed afterwards, and one app SQL wrote
//! while the org's `app_events` flag was off, once the flag is on.
//!
//! A host publish relaying as before is `app_events_publish`'s
//! (`a_host_publish_is_signed_and_relays`); every honest row here is signed
//! as the host signs one ([`Relay::host_mac`]).

use super::*;
use oxy_api_custom_apps::app_events::provenance::UNVERIFIED_PROVENANCE;

/// `id`'s stored event, refused for its provenance.
async fn assert_quarantined(r: &Relay, id: Uuid) {
    let event = app_events::Entity::find_by_id(id)
        .one(&r.t.db)
        .await
        .expect("read the event")
        .unwrap_or_else(|| panic!("event {id} is stored"));
    let error = event.relay_error.as_deref().expect("a relay error");
    assert!(
        error.starts_with(&format!("{UNVERIFIED_PROVENANCE}: ")),
        "{error}"
    );
    let delivered = r
        .deliveries()
        .await
        .into_iter()
        .filter(|d| d.event_id == id)
        .count();
    assert_eq!(delivered, 0, "a quarantined event reaches nobody");
    assert_eq!(bodies(r, id).await, 1, "its body is kept, never deleted");
}

/// A row an honest host would write for `key`.
async fn honest_row(r: &Relay, key: &'static str) -> Row<'static> {
    let revision = live_revision(&r.t.db, r.publisher, "production", EVENT)
        .await
        .expect("read the live revision")
        .map(|rev| rev.id);
    Row {
        id: event_id(r.org(), "production", &r.event_type(EVENT), key),
        environment: "production",
        name: EVENT,
        key,
        revision,
        delay: 0,
        data: json!({ "period": "2026-11" }),
    }
}

/// App SQL calling the definer with a forged actor, or a forged root, cannot
/// sign it: a MAC copied from an honest row's fields, or one made up, does
/// not match, and neither row reaches the listener.
#[tokio::test]
async fn app_sql_forging_the_actor_or_the_root_is_quarantined() {
    with_relay(|r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let conn = r.publishable_writer(&r.store.writer).await;

        let forged_actor = honest_row(&r, "statement:actor").await;
        let honest = Lineage::top_level(forged_actor.id);
        let mac = r.host_mac(&r.store.writer, &conn.role, &forged_actor, &honest);
        let lineage = Lineage {
            actor: Some("user:someone-else"),
            ..Lineage::top_level(forged_actor.id)
        };
        r.call_publish(&conn, &forged_actor, &lineage, &mac).await;

        let forged_root = honest_row(&r, "statement:root").await;
        let lineage = Lineage {
            root: Uuid::new_v4(),
            ..Lineage::top_level(forged_root.id)
        };
        r.call_publish(&conn, &forged_root, &lineage, &hex::encode([0u8; 32]))
            .await;

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 0,
                deliveries: 0,
                rejected: 2
            }
        );
        assert_quarantined(&r, forged_actor.id).await;
        assert_quarantined(&r, forged_root.id).await;
        assert!(r.event_tasks().await.is_empty());
        assert_eq!(r.waiting().await, 0, "both left the claim set");
    })
    .await;
}

/// A row the host signed whose `causation_id` changed afterwards (here by
/// the store's owner, the only role that can) no longer matches its MAC.
#[tokio::test]
async fn a_signed_row_whose_causation_changed_afterwards_is_quarantined() {
    with_relay(|r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let id = r.publish_event("production", EVENT, "statement:1", 0).await;
        owner_of(&r)
            .await
            .execute(
                "UPDATE oxy_events.outbox SET causation_id = $1 WHERE id = $2",
                &[&Uuid::new_v4(), &id],
            )
            .await
            .expect("change the causation");

        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(stats.rejected, 1, "{stats:?}");
        assert_eq!(stats.relayed, 0, "{stats:?}");
        assert_quarantined(&r, id).await;
    })
    .await;
}

/// The host refuses a publish while the flag is off, so it never signs one;
/// a row app SQL wrote then waits, inert, and once the flag is on it is
/// relayed into quarantine, never delivered.
#[tokio::test]
async fn a_row_app_sql_wrote_while_the_flag_was_off_is_quarantined_once_it_is_on() {
    with_relay_flag(false, |r| async move {
        r.listener("relay-a", &[r.event_type(EVENT)]).await;
        let conn = r.publishable_writer(&r.store.writer).await;
        let row = honest_row(&r, "statement:early").await;
        r.call_publish(
            &conn,
            &row,
            &Lineage::top_level(row.id),
            &hex::encode([7u8; 32]),
        )
        .await;
        let off = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("a flag-off pass succeeds");
        assert_eq!(off, RelayStats::default());
        assert_eq!(r.waiting().await, 1, "inert while the flag is off");

        crate::app_events_publish::enable_app_events(&r.t, &[r.org()]).await;
        let stats = relay_once(&r.t.db, r.org(), "production")
            .await
            .expect("relay");
        assert_eq!(
            stats,
            RelayStats {
                relayed: 0,
                deliveries: 0,
                rejected: 1
            }
        );
        assert_quarantined(&r, row.id).await;
    })
    .await;
}
