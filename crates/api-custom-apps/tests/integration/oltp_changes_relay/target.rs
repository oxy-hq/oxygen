//! A writer held for its target (design §5.3): its app's workspace has no
//! Airhouse, so the pass holds the writer and leaves its rows in the log.
//! Each later pass looks again; once the Airhouse exists, the pass lifts the
//! hold, ships what waited, and begins the baseline that waited behind it.

use std::sync::Arc;

use oxy_api_custom_apps::oltp_changes::relay::RelayStats;
use oxy_api_custom_apps::oltp_changes::relay::sink::install_sink;

use super::duck::AirhouseLater;
use super::{text, with_relay};
use crate::oltp_changes_fixture::LEASES_SQL;

/// `leases` captured and one row written while the app's workspace has no
/// Airhouse: the pass holds the writer for its target, and a pass before the
/// Airhouse exists keeps the hold, with its first reason. Nothing ships,
/// nothing is made in Airhouse, and the install's baseline waits. Once the
/// Airhouse exists, the next pass lifts the hold, ships the row, and begins
/// the baseline; later passes finish it, and the view shows the row.
#[tokio::test]
async fn a_writer_held_for_its_target_ships_once_its_airhouse_exists() {
    with_relay(|r| async move {
        let sink = Arc::new(AirhouseLater::new(r.duck.clone()));
        install_sink(sink.clone());
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        assert_eq!(r.waiting(None).await, 1);

        let held = RelayStats {
            held: 1,
            ..RelayStats::default()
        };
        assert_eq!(r.pass().await, held, "no Airhouse: the writer is held");
        let reason = format!(
            "app {} has no Airhouse in workspace {}",
            r.app.slug, r.app.workspace
        );
        let hold = (r.writer(), String::new(), reason);
        assert_eq!(r.holds().await, [hold.clone()]);
        assert_eq!(r.pass().await, held, "still no Airhouse: still held");
        assert_eq!(r.holds().await, [hold], "one hold, with its first reason");
        assert_eq!(r.waiting(None).await, 1, "the row stays in the log");
        assert!(r.tables().is_empty(), "nothing made in Airhouse");
        let requested = (
            "leases".to_owned(),
            "opt_in".to_owned(),
            "requested".to_owned(),
        );
        assert_eq!(r.baselines().await, [requested], "the baseline waits");

        sink.make();
        let lifted = r.pass().await;
        assert_eq!(
            (lifted.shipped, lifted.batches, lifted.held),
            (1, 1, 0),
            "the hold is lifted and the row ships: {lifted:?}"
        );
        assert_eq!(lifted.begun, 1, "its baseline begins in the same pass");
        let holds = r.holds().await;
        assert!(holds.is_empty(), "the hold is gone: {holds:?}");
        assert_eq!(
            r.changes("leases", "id, name"),
            [[text("1"), text("Flat 1")]]
        );

        r.settle().await;
        assert_eq!(r.view("leases"), [[text("1"), text("Flat 1")]]);
    })
    .await;
}
