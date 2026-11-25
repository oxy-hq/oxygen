//! The KMS-sealed master key against a real Postgres, with a fake KMS: the
//! first boot of a pod whose namespace has the KMS alias seals
//! `OXY_ENCRYPTION_KEY` into `server_keys` only after it opens the data
//! already there, and later boots take the key from the row.
//!
//! The boot rule is unit-tested with a fake store in
//! `oxy_platform::secrets::master_key`. These pin the SQL and the hand-off to
//! the encryption-key check. Nothing here touches the process environment or
//! installs a key: `resolve` is called directly.
//!
//! Database-backed through [`crate::common::fresh_db`], so `db-per-test`.

use entity::organizations;
use oxy::adapters::secrets::envelope;
use oxy::adapters::secrets::key_check::CHECK_ROW;
use oxy::adapters::secrets::master_key::{
    KeyLookup, Kms, MASTER_KEY_ROW, PostgresStore, Resolved, StoredMasterKey, resolve,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

/// The pod's namespace, and the alias and key it names.
const NS: &str = "oxy-test";
const ALIAS: &str = "alias/oxy-test-oxy-master-key";
const ARN: &str = "arn:aws:kms:us-west-2:1:key/test";
const A: [u8; 32] = [5; 32];
const B: [u8; 32] = [6; 32];

/// Knows [`ALIAS`], naming [`ARN`]. Seals by XOR, tagged with the ARN, so a
/// blob opens with no key id and says which key opened it.
struct FakeKms;

impl Kms for FakeKms {
    async fn find_key(&self, key_id: &str) -> Result<KeyLookup, String> {
        Ok(if key_id == ALIAS {
            KeyLookup::Found
        } else {
            KeyLookup::NotFound
        })
    }

    async fn encrypt(&self, key_id: &str, plaintext: &[u8]) -> Result<(String, Vec<u8>), String> {
        assert_eq!(key_id, ALIAS);
        let mut ct = ARN.as_bytes().to_vec();
        ct.push(0);
        ct.extend(plaintext.iter().map(|b| b ^ 0xa5));
        Ok((ARN.to_string(), ct))
    }

    async fn decrypt(
        &self,
        _sealed_by: &str,
        ciphertext: &[u8],
    ) -> Result<(String, Vec<u8>), String> {
        let split = ciphertext.iter().position(|b| *b == 0).ok_or("not ours")?;
        let arn = String::from_utf8(ciphertext[..split].to_vec()).map_err(|e| e.to_string())?;
        Ok((
            arn,
            ciphertext[split + 1..].iter().map(|b| b ^ 0xa5).collect(),
        ))
    }
}

fn stored(key: [u8; 32]) -> Result<Resolved, String> {
    Ok(Resolved::Stored {
        key,
        sealed_by: ARN.to_string(),
    })
}

async fn db() -> DatabaseConnection {
    crate::common::fresh_db(crate::common::Schema::Central)
        .await
        .0
}

async fn row(db: &DatabaseConnection, name: &str) -> Option<Vec<u8>> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT secret FROM server_keys WHERE name = $1",
        [name.into()],
    ))
    .await
    .expect("read server_keys")
    .map(|r| r.try_get::<Vec<u8>>("", "secret").expect("secret"))
}

async fn seed_org_secret(db: &DatabaseConnection, key: &[u8; 32]) {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("kms".into()),
        slug: ActiveValue::Set(format!("kms-{}", org.simple())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let sealed = envelope::seal_with_key(key, b"a token").unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO org_secrets (id, org_id, name, ciphertext) VALUES ($1, $2, 't', $3)",
        [Uuid::new_v4().into(), org.into(), sealed.into()],
    ))
    .await
    .expect("seed org secret");
}

#[tokio::test]
async fn the_first_boot_seals_the_variable_and_later_boots_need_only_the_row() {
    let db = db().await;
    seed_org_secret(&db, &A).await;
    let store = PostgresStore { db: &db };

    assert_eq!(
        resolve(&FakeKms, &store, Some(NS), true, Some(A)).await,
        stored(A)
    );
    let row_written =
        StoredMasterKey::from_bytes(&row(&db, MASTER_KEY_ROW).await.expect("row written"))
            .expect("row format");
    assert_eq!(row_written.sealed_by, ARN, "{row_written:?}");
    // The data check ran on the way, and recorded the check value.
    assert!(row(&db, CHECK_ROW).await.is_some());

    // The variable removed: the row alone is the key.
    assert_eq!(
        resolve(&FakeKms, &store, Some(NS), true, None).await,
        stored(A)
    );

    // A variable that is not the stored key is refused, naming both.
    let err = resolve(&FakeKms, &store, Some(NS), true, Some(B))
        .await
        .unwrap_err();
    assert!(
        err.contains("OXY_ENCRYPTION_KEY is set and is not the master key"),
        "{err}"
    );
}

#[tokio::test]
async fn a_variable_that_does_not_open_the_data_is_never_sealed_into_the_row() {
    let db = db().await;
    seed_org_secret(&db, &A).await;
    let store = PostgresStore { db: &db };

    let err = resolve(&FakeKms, &store, Some(NS), true, Some(B))
        .await
        .unwrap_err();
    assert!(err.contains("opens none of the sealed values"), "{err}");
    assert_eq!(row(&db, MASTER_KEY_ROW).await, None);
    assert_eq!(row(&db, CHECK_ROW).await, None);
}

#[tokio::test]
async fn no_row_and_no_variable_refuses_without_making_a_key() {
    let db = db().await;
    let err = resolve(&FakeKms, &PostgresStore { db: &db }, Some(NS), true, None)
        .await
        .unwrap_err();
    assert!(err.contains("needs OXY_ENCRYPTION_KEY"), "{err}");
    assert_eq!(row(&db, MASTER_KEY_ROW).await, None);
}

#[tokio::test]
async fn a_namespace_without_the_alias_writes_nothing() {
    let db = db().await;
    let got = resolve(
        &FakeKms,
        &PostgresStore { db: &db },
        Some("no-such-namespace"),
        true,
        Some(A),
    )
    .await;
    let Ok(Resolved::NotSealed { warning: Some(w) }) = got else {
        panic!("expected the variable, got {got:?}");
    };
    assert!(
        w.contains("alias/no-such-namespace-oxy-master-key does not exist"),
        "{w}"
    );
    assert_eq!(row(&db, MASTER_KEY_ROW).await, None);
    assert_eq!(row(&db, CHECK_ROW).await, None);
}
