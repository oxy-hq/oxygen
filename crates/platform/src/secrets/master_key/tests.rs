//! The boot rule with a fake KMS and a fake store.

use super::row::ROW_FORMAT;
use super::*;
use std::sync::Mutex;

/// Seals by XOR with a byte per key id, so a ciphertext only "decrypts"
/// under the key that made it. `fail` makes every call fail, as KMS does
/// when the role lacks access.
struct FakeKms {
    fail: Option<&'static str>,
}

fn pad(key_id: &str) -> u8 {
    key_id.bytes().fold(0x5a, |a, b| a ^ b)
}

impl Kms for FakeKms {
    async fn encrypt(&self, key_id: &str, plaintext: &[u8]) -> Result<(String, Vec<u8>), String> {
        if let Some(e) = self.fail {
            return Err(e.into());
        }
        let mut ct = key_id.as_bytes().to_vec();
        ct.push(0);
        ct.extend(plaintext.iter().map(|b| b ^ pad(key_id)));
        Ok((format!("arn:aws:kms:us-west-2:1:key/{key_id}"), ct))
    }

    async fn decrypt(&self, key_id: &str, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        if let Some(e) = self.fail {
            return Err(e.into());
        }
        let split = ciphertext.iter().position(|b| *b == 0).ok_or("not ours")?;
        if &ciphertext[..split] != key_id.as_bytes() {
            return Err("IncorrectKeyException".into());
        }
        Ok(ciphertext[split + 1..]
            .iter()
            .map(|b| b ^ pad(key_id))
            .collect())
    }
}

#[derive(Default)]
struct FakeStore {
    row: Mutex<Option<Vec<u8>>>,
    /// A row another process writes between our insert and read-back.
    raced: Option<Vec<u8>>,
    variable_fails: bool,
    verified: Mutex<u32>,
}

impl MasterKeyStore for FakeStore {
    async fn read(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.row.lock().unwrap().clone())
    }
    async fn insert_if_absent(&self, row: &[u8]) -> Result<(), String> {
        let mut slot = self.row.lock().unwrap();
        if slot.is_none() {
            *slot = Some(self.raced.clone().unwrap_or_else(|| row.to_vec()));
        }
        Ok(())
    }
    async fn verify_variable(&self, _key: &[u8; 32]) -> Result<(), String> {
        *self.verified.lock().unwrap() += 1;
        if self.variable_fails {
            Err("opens none of the sealed values".into())
        } else {
            Ok(())
        }
    }
}

const ID: &str = "alias/oxy-test-master-key";
const A: [u8; 32] = [3; 32];
const B: [u8; 32] = [4; 32];
const OK: FakeKms = FakeKms { fail: None };

async fn row_for(key: [u8; 32]) -> Vec<u8> {
    let (sealed_by, ciphertext) = OK.encrypt(ID, &key).await.unwrap();
    StoredMasterKey {
        sealed_by,
        ciphertext,
    }
    .to_bytes()
}

#[tokio::test]
async fn a_present_row_is_the_key_and_needs_no_variable() {
    let store = FakeStore {
        row: Mutex::new(Some(row_for(A).await)),
        ..Default::default()
    };
    assert_eq!(resolve(&OK, &store, ID, None).await, Ok(A));
    assert_eq!(resolve(&OK, &store, ID, Some(A)).await, Ok(A));
    assert_eq!(
        *store.verified.lock().unwrap(),
        0,
        "a present row is not re-checked against data"
    );
}

#[tokio::test]
async fn a_variable_that_differs_from_the_row_is_refused_naming_both() {
    let store = FakeStore {
        row: Mutex::new(Some(row_for(A).await)),
        ..Default::default()
    };
    let err = resolve(&OK, &store, ID, Some(B)).await.unwrap_err();
    assert!(
        err.contains("OXY_ENCRYPTION_KEY is set and is not the master key"),
        "{err}"
    );
    assert!(err.contains("master_key") && err.contains(ID), "{err}");
}

#[tokio::test]
async fn no_row_seals_the_variable_after_it_passes_the_data_check() {
    let store = FakeStore::default();
    assert_eq!(resolve(&OK, &store, ID, Some(A)).await, Ok(A));
    assert_eq!(*store.verified.lock().unwrap(), 1);
    let stored = StoredMasterKey::from_bytes(&store.row.lock().unwrap().clone().unwrap()).unwrap();
    assert!(stored.sealed_by.starts_with("arn:aws:kms:"), "{stored:?}");
    // The next boot needs no variable.
    assert_eq!(resolve(&OK, &store, ID, None).await, Ok(A));
}

#[tokio::test]
async fn no_row_and_no_variable_is_refused_and_nothing_is_made() {
    let store = FakeStore::default();
    let err = resolve(&OK, &store, ID, None).await.unwrap_err();
    assert!(err.contains("needs OXY_ENCRYPTION_KEY"), "{err}");
    assert!(store.row.lock().unwrap().is_none());
}

#[tokio::test]
async fn a_variable_that_fails_the_data_check_is_not_recorded() {
    let store = FakeStore {
        variable_fails: true,
        ..Default::default()
    };
    let err = resolve(&OK, &store, ID, Some(B)).await.unwrap_err();
    assert!(err.contains("opens none"), "{err}");
    assert!(store.row.lock().unwrap().is_none());
}

#[tokio::test]
async fn losing_the_race_to_a_different_key_is_refused() {
    let store = FakeStore {
        raced: Some(row_for(A).await),
        ..Default::default()
    };
    let err = resolve(&OK, &store, ID, Some(B)).await.unwrap_err();
    assert!(
        err.contains("Another process recorded a different master key"),
        "{err}"
    );
    // Losing it to the same key is fine.
    let store = FakeStore {
        raced: Some(row_for(A).await),
        ..Default::default()
    };
    assert_eq!(resolve(&OK, &store, ID, Some(A)).await, Ok(A));
}

#[tokio::test]
async fn kms_failure_is_refused_naming_the_key_and_the_role_with_no_fallback() {
    let denied = FakeKms {
        fail: Some("AccessDeniedException: not authorized"),
    };
    for store in [
        FakeStore {
            row: Mutex::new(Some(row_for(A).await)),
            ..Default::default()
        },
        FakeStore::default(),
    ] {
        let err = resolve(&denied, &store, ID, Some(A)).await.unwrap_err();
        assert!(err.contains(ID), "{err}");
        assert!(err.contains("AccessDeniedException"), "{err}");
        assert!(err.contains("EKS Pod Identity"), "{err}");
    }
    // A row sealed by another KMS key does not open under this one.
    let store = FakeStore {
        row: Mutex::new(Some(row_for(A).await)),
        ..Default::default()
    };
    let err = resolve(&OK, &store, "alias/another", None)
        .await
        .unwrap_err();
    assert!(err.contains("Could not decrypt"), "{err}");
}

#[test]
fn the_row_format_round_trips_and_refuses_anything_else() {
    let stored = StoredMasterKey {
        sealed_by: "arn:aws:kms:us-west-2:1:key/k".into(),
        ciphertext: vec![1, 2, 3],
    };
    assert_eq!(StoredMasterKey::from_bytes(&stored.to_bytes()), Ok(stored));
    assert!(StoredMasterKey::from_bytes(&[0; 32]).is_err());
    assert!(StoredMasterKey::from_bytes(ROW_FORMAT).is_err());
}
