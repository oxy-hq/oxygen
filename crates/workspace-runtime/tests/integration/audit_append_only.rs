//! `audit_events` is append-only at the database (migration
//! `m20260908_000001_audit_events_append_only`): UPDATE and TRUNCATE always
//! fail, DELETE fails unless the session carries the retention prune's flag. Pinned
//! against the live trigger rather than the migration source, because the
//! thing worth pinning is what Postgres *does* to a hand-edit.

use oxy::database::client::establish_connection;
use oxy_app_core::audit::{self, AuditEntry};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
use uuid::Uuid;

fn db_unavailable() -> bool {
    std::env::var("OXY_DATABASE_URL").is_err()
}

#[tokio::test]
async fn a_hand_edit_or_delete_of_an_audit_row_is_refused_by_the_database() {
    if db_unavailable() {
        eprintln!("skipping: OXY_DATABASE_URL not set");
        return;
    }
    let db = establish_connection().await.expect("db");
    let org = Uuid::new_v4();
    let id = audit::record(
        &db,
        AuditEntry::new("append-only@audit.test", "test.append_only").org(org),
    )
    .await
    .expect("record");

    let update = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE audit_events SET reason = 'edited' WHERE id = $1",
            [id.into()],
        ))
        .await;
    let err = update.expect_err("UPDATE must be refused").to_string();
    assert!(err.contains("append-only"), "{err}");

    let delete = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM audit_events WHERE id = $1",
            [id.into()],
        ))
        .await;
    let err = delete.expect_err("DELETE must be refused").to_string();
    assert!(err.contains("append-only"), "{err}");

    // TRUNCATE never fires a row-level trigger; the statement-level one refuses.
    let truncate = db.execute_unprepared("TRUNCATE audit_events").await;
    let err = truncate.expect_err("TRUNCATE must be refused").to_string();
    assert!(err.contains("append-only"), "{err}");

    // The prune's flag opens DELETE for that transaction only. Rolled back so
    // the shared test database keeps the row and the chain stays intact.
    let txn = db.begin().await.expect("begin");
    txn.execute_unprepared(&format!("SET LOCAL {} = 'on'", audit::PRUNE_SETTING))
        .await
        .expect("set flag");
    let res = txn
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM audit_events WHERE id = $1",
            [id.into()],
        ))
        .await
        .expect("DELETE with the prune flag");
    assert_eq!(res.rows_affected(), 1);
    txn.rollback().await.expect("rollback");

    let report = audit::verify_chain(&db, org).await.expect("verify");
    assert!(report.intact, "{report:?}");
    assert_eq!(report.events, 1);
}
