//! Drift repair (design §5.2, layer 3): a capture trigger the app disabled
//! is put back by the next pass, and the rows written meanwhile reach
//! Airhouse through a `drift` baseline; a store without its DDL guard is
//! recorded so once; and a repair waits for an install that holds the app's
//! install lock. A repair never adds or removes capture of a table: a pass
//! between a promote's install and its pointer move, with the old build
//! still declared, neither captures again a table the new build removed nor
//! removes one it added. The local provider's owner may not create the guard,
//! so the app's writer can disable its own capture here.

use oxy_api_custom_apps::oltp_changes::install::install_on_promote;
use oxy_custom_apps::manifest::OltpChangesConfig;
use oxy_oltp::changes::INSTALL_LOCK_SQL;
use serde_json::json;

use super::{Relay, text, with_relay};
use crate::oltp_changes_fixture::{LEASES_SQL, NOTES_SQL, connect};

/// Every capture trigger on `leases` enabled always, by name.
const IN_PLACE: [(&str, &str); 4] = [
    ("oxy_capture_d", "A"),
    ("oxy_capture_i", "A"),
    ("oxy_capture_t", "A"),
    ("oxy_capture_u", "A"),
];

impl Relay {
    /// A promote's install of a build declaring `tables`, alone: its pointer
    /// does not move, so production still serves the build before.
    async fn install_without_promoting(&self, tables: &[&str]) {
        let declared: serde_json::Map<String, serde_json::Value> = tables
            .iter()
            .map(|table| (table.to_string(), json!({})))
            .collect();
        let config: OltpChangesConfig =
            serde_json::from_value(json!({ "tables": declared })).expect("an oltpChanges block");
        install_on_promote(self.db(), self.app.org(), &self.app.slug, Some(&config))
            .await
            .expect("the install");
    }

    /// Whether the store captures the writer's `table`: it has its
    /// `oxy_changes.tables` row.
    async fn has_capture(&self, table: &str) -> bool {
        self.app
            .owner()
            .await
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM oxy_changes.tables \
                  WHERE writer = $1 AND table_name = $2)",
                &[&self.writer(), &table],
            )
            .await
            .expect("read the captured tables")
            .get(0)
    }

    /// Capture installed on `leases`, and settled.
    async fn captured_leases(&self) {
        self.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        self.settle().await;
    }

    /// `sql` on a session as the app's writer, which owns `leases`.
    async fn as_app_writer(&self, sql: &str) {
        connect(&self.app.writer().await.dsn)
            .await
            .batch_execute(sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {}", oxy_oltp::connect::pg_detail(&e)));
    }

    /// Each capture trigger on `leases` as `(tgname, tgenabled)`, by name.
    async fn capture_triggers(&self) -> Vec<(String, String)> {
        let leases = format!("{}.leases", self.app.schema());
        let rows = self
            .app
            .owner()
            .await
            .query(
                "SELECT tgname::text, tgenabled::text FROM pg_trigger \
                  WHERE tgrelid = $1::text::regclass AND tgname LIKE 'oxy\\_capture\\_%' \
                  ORDER BY tgname",
                &[&leases],
            )
            .await
            .expect("read the capture triggers");
        rows.iter().map(|row| (row.get(0), row.get(1))).collect()
    }

    /// The stream's `guardless_since`.
    async fn guardless_since(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.app
            .owner()
            .await
            .query_one("SELECT guardless_since FROM oxy_changes.stream", &[])
            .await
            .expect("read the stream")
            .get(0)
    }
}

fn in_place() -> Vec<(String, String)> {
    IN_PLACE
        .iter()
        .map(|(name, enabled)| ((*name).to_owned(), (*enabled).to_owned()))
        .collect()
}

/// The insert trigger disabled, then row 7 written uncaptured: the next pass
/// puts the trigger back and requests a `drift` baseline, which, settled,
/// brings row 7 into the view.
#[tokio::test]
async fn a_disabled_trigger_is_put_back_and_the_gap_baselined() {
    with_relay(|r| async move {
        r.captured_leases().await;
        r.as_app_writer("ALTER TABLE leases DISABLE TRIGGER oxy_capture_i")
            .await;
        r.as_app_writer("INSERT INTO leases (id, name) VALUES (7, 'Flat 7')")
            .await;
        assert_eq!(r.waiting(Some("leases")).await, 0, "row 7 is not captured");

        r.pass().await;
        r.settle().await;
        assert_eq!(r.capture_triggers().await, in_place());
        let baselines = r.baselines().await;
        let drift = (
            "leases".to_owned(),
            "drift".to_owned(),
            "complete".to_owned(),
        );
        assert!(baselines.contains(&drift), "{baselines:?}");
        let ids: Vec<Option<String>> = r
            .view("leases")
            .into_iter()
            .map(|row| row[0].clone())
            .collect();
        assert!(ids.contains(&text("7")), "{ids:?}");
    })
    .await;
}

/// The local provider's owner may not create the guard: the first pass
/// records the store guardless, and the next leaves the record as it was.
#[tokio::test]
async fn a_missing_guard_is_recorded_once() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        assert_eq!(r.guardless_since().await, None, "nothing recorded yet");

        r.pass().await;
        let first = r.guardless_since().await;
        assert!(first.is_some(), "the first pass records it");

        r.pass().await;
        assert_eq!(r.guardless_since().await, first, "recorded once");
    })
    .await;
}

/// An install holding the app's lock: the pass leaves the disabled trigger
/// for it, and does not fail; the lock released, the next pass repairs it.
#[tokio::test]
async fn a_repair_waits_for_an_install_in_progress() {
    with_relay(|r| async move {
        r.captured_leases().await;
        let install = r.app.owner().await;
        install
            .execute(INSTALL_LOCK_SQL, &[&r.writer()])
            .await
            .expect("take the app's install lock");
        r.as_app_writer("ALTER TABLE leases DISABLE TRIGGER oxy_capture_i")
            .await;

        r.pass().await;
        let mut disabled = in_place();
        disabled[1].1 = "D".to_owned();
        assert_eq!(r.capture_triggers().await, disabled, "left for the install");

        let released: bool = install
            .query_one(
                "SELECT pg_advisory_unlock(hashtextextended('oxy_changes.install:' || $1::text, 0))",
                &[&r.writer()],
            )
            .await
            .expect("release the app's install lock")
            .get(0);
        assert!(released);
        r.pass().await;
        assert_eq!(r.capture_triggers().await, in_place(), "repaired");
    })
    .await;
}

/// Production declares `leases` and `notes`; a promote's install of a build
/// declaring `leases` alone has removed `notes`, and its pointer has not
/// moved. A pass in between, with `leases`' insert trigger disabled, repairs
/// `leases` and records `notes`, declared but uncaptured, without capturing
/// it again.
#[tokio::test]
async fn a_pass_before_the_pointer_moves_keeps_a_removal() {
    with_relay(|r| async move {
        r.app
            .promote_declaring("chg-1", &["leases", "notes"], &[LEASES_SQL, NOTES_SQL])
            .await
            .expect("the promote installs capture on both");
        r.settle().await;
        r.install_without_promoting(&["leases"]).await;
        assert!(!r.has_capture("notes").await, "the install removed notes");
        r.as_app_writer("ALTER TABLE leases DISABLE TRIGGER oxy_capture_i")
            .await;

        r.pass().await;
        assert!(!r.has_capture("notes").await, "notes stays uncaptured");
        assert_eq!(r.capture_triggers().await, in_place(), "leases is repaired");
    })
    .await;
}

/// Production declares `leases`; a promote's install of a build declaring
/// `notes` too has captured it, and its pointer has not moved. A pass in
/// between, with `leases`' insert trigger disabled, repairs `leases` and
/// leaves `notes` captured.
#[tokio::test]
async fn a_pass_before_the_pointer_moves_keeps_an_addition() {
    with_relay(|r| async move {
        r.app
            .promote_declaring("chg-1", &["leases"], &[LEASES_SQL, NOTES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.settle().await;
        r.install_without_promoting(&["leases", "notes"]).await;
        assert!(r.has_capture("notes").await, "the install captured notes");
        r.as_app_writer("ALTER TABLE leases DISABLE TRIGGER oxy_capture_i")
            .await;

        r.pass().await;
        assert!(r.has_capture("notes").await, "notes stays captured");
        assert_eq!(r.capture_triggers().await, in_place(), "leases is repaired");
    })
    .await;
}
