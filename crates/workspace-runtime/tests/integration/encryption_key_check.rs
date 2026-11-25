//! The encryption-key boot check against a real Postgres: the check value in
//! `server_keys`, and the first-boot rule for a deployment that already has
//! sealed data and no check value.
//!
//! The rule itself is unit-tested in `oxy_platform::secrets::key_check`. These
//! pin the SQL it runs: the row it writes, the tables it samples, and that a
//! missing table (no airhouse or camera migrator in `Schema::Central`) is
//! skipped rather than an error. Keys are passed in, so nothing here touches
//! the process environment.
//!
//! Database-backed through [`crate::common::fresh_db`], so `db-per-test`.

use entity::organizations;
use oxy::adapters::secrets::{envelope, key_check};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

const KEY: [u8; 32] = [1; 32];
const OTHER: [u8; 32] = [2; 32];

async fn db() -> DatabaseConnection {
    crate::common::fresh_db(crate::common::Schema::Central)
        .await
        .0
}

async fn check_row(db: &DatabaseConnection) -> Option<Vec<u8>> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT secret FROM server_keys WHERE name = $1",
        [key_check::CHECK_ROW.into()],
    ))
    .await
    .expect("read server_keys")
    .map(|row| row.try_get::<Vec<u8>>("", "secret").expect("secret"))
}

/// One org secret sealed under `key`, as `OrgSecretsService` would store it.
async fn seed_org_secret(db: &DatabaseConnection, key: &[u8; 32]) {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("key-check".into()),
        slug: ActiveValue::Set(format!("key-check-{}", org.simple())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let sealed = envelope::seal_with_key(key, b"a slack token").unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO org_secrets (id, org_id, name, ciphertext) VALUES ($1, $2, 'slack', $3)",
        [Uuid::new_v4().into(), org.into(), sealed.into()],
    ))
    .await
    .expect("seed org secret");
}

#[tokio::test]
async fn a_fresh_database_gets_a_check_value_and_then_holds_every_boot_to_it() {
    let db = db().await;
    assert_eq!(check_row(&db).await, None);

    key_check::verify_key(&db, &KEY, "test key")
        .await
        .expect("first boot writes the row");
    let sealed = check_row(&db).await.expect("the row was written");
    assert_eq!(
        envelope::open_with_key(&KEY, &sealed).unwrap(),
        key_check::CHECK_PLAINTEXT
    );

    // The same key again: proceeds, and leaves the row as it was.
    key_check::verify_key(&db, &KEY, "test key")
        .await
        .expect("same key");
    assert_eq!(check_row(&db).await.as_deref(), Some(sealed.as_slice()));

    // Another key: refused, naming where it came from and the row.
    let err = key_check::verify_key(&db, &OTHER, "OXY_ENCRYPTION_KEY")
        .await
        .expect_err("a different key must be refused");
    assert!(
        err.contains(&format!(
            "from OXY_ENCRYPTION_KEY (fingerprint {}) does not open",
            key_check::fingerprint(&OTHER)
        )),
        "{err}"
    );
    assert!(err.contains(key_check::CHECK_ROW), "{err}");
    assert_eq!(check_row(&db).await.as_deref(), Some(sealed.as_slice()));
}

#[tokio::test]
async fn existing_data_under_another_key_is_refused_and_no_row_is_written() {
    let db = db().await;
    seed_org_secret(&db, &KEY).await;

    let err = key_check::verify_key(&db, &OTHER, "OXY_ENCRYPTION_KEY")
        .await
        .expect_err("a key that opens none of the data must be refused");
    assert!(err.contains("opens none of the sealed values"), "{err}");
    assert!(err.contains("org_secrets.ciphertext"), "{err}");
    assert_eq!(
        check_row(&db).await,
        None,
        "nothing written under the wrong key"
    );

    // The key the data was sealed with records the row, after which the
    // wrong one meets the check value instead.
    key_check::verify_key(&db, &KEY, "OXY_ENCRYPTION_KEY")
        .await
        .expect("the right key passes");
    assert!(check_row(&db).await.is_some());
    let err = key_check::verify_key(&db, &OTHER, "OXY_ENCRYPTION_KEY")
        .await
        .expect_err("still refused");
    assert!(
        err.contains("does not open this database's encryption check value"),
        "{err}"
    );
}

/// Staging, 2026-10-28: one workspace's `_var` secrets were sealed under
/// another key and copied in, while every pod holds the same key and the rest
/// opens. The first boot starts and writes the row; the unreadable rows are
/// logged, not a reason for an outage.
#[tokio::test]
async fn mixed_data_starts_and_records_the_key() {
    let db = db().await;
    seed_org_secret(&db, &KEY).await;
    // A legacy `_var` secret under another key: base64 of the envelope, as
    // `SecretManagerService` stores it.
    let old = envelope::seal_with_key(&OTHER, b"old").unwrap();
    let user = Uuid::new_v4();
    let workspace = Uuid::new_v4();
    db.execute_unprepared(&format!(
        "SET session_replication_role = replica; \
         INSERT INTO secrets (id, name, encrypted_value, created_by, project_id, is_active) \
         VALUES ('{}', 'CLICKHOUSE_PASSWORD', '{}', '{user}', '{workspace}', true); \
         SET session_replication_role = DEFAULT;",
        Uuid::new_v4(),
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &old),
    ))
    .await
    .expect("seed legacy secret");

    key_check::verify_key(&db, &KEY, "OXY_ENCRYPTION_KEY")
        .await
        .expect("a key that opens some of the data starts");
    assert!(check_row(&db).await.is_some());

    // The other key opens only the legacy row, and now meets the check value.
    let err = key_check::verify_key(&db, &OTHER, "OXY_ENCRYPTION_KEY")
        .await
        .expect_err("refused");
    assert!(
        err.contains("does not open this database's encryption check value"),
        "{err}"
    );
}
