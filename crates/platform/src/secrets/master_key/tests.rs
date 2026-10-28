//! The boot rule with a fake KMS and a fake store.

use super::row::ROW_FORMAT;
use super::*;
use std::sync::Mutex;

/// The alias of [`NS`], and the ARN of the key it names.
const NS: &str = "oxy-test";
const ALIAS: &str = "alias/oxy-test-oxy-master-key";
const ARN: &str = "arn:aws:kms:us-west-2:1:key/test";

/// Seals by XOR with a byte per key ARN, tagging the ciphertext with that ARN,
/// so a ciphertext "decrypts" with no key id and says which key opened it.
/// Knows one alias, [`ALIAS`], naming [`ARN`].
struct FakeKms {
    /// What `DescribeKey` on [`ALIAS`] answers; `Err` is the text of the
    /// failure.
    lookup: Result<KeyLookup, &'static str>,
    /// When set, every Encrypt and Decrypt fails with it, as KMS does when the
    /// role lacks access.
    fail: Option<&'static str>,
    /// When set, Decrypt reports this key rather than the one in the blob.
    opened_by: Option<&'static str>,
    /// The KMS calls made, in order.
    calls: Mutex<Vec<&'static str>>,
}

impl Default for FakeKms {
    fn default() -> Self {
        Self {
            lookup: Ok(KeyLookup::Found),
            fail: None,
            opened_by: None,
            calls: Mutex::default(),
        }
    }
}

fn pad(arn: &str) -> u8 {
    arn.bytes().fold(0x5a, |a, b| a ^ b)
}

fn seal(arn: &str, plaintext: &[u8]) -> Vec<u8> {
    let mut ct = arn.as_bytes().to_vec();
    ct.push(0);
    ct.extend(plaintext.iter().map(|b| b ^ pad(arn)));
    ct
}

impl Kms for FakeKms {
    async fn find_key(&self, key_id: &str) -> Result<KeyLookup, String> {
        self.calls.lock().unwrap().push("DescribeKey");
        assert_eq!(key_id, ALIAS, "the alias comes from the namespace");
        self.lookup.clone().map_err(str::to_string)
    }

    async fn encrypt(&self, key_id: &str, plaintext: &[u8]) -> Result<(String, Vec<u8>), String> {
        self.calls.lock().unwrap().push("Encrypt");
        if let Some(e) = self.fail {
            return Err(e.into());
        }
        assert_eq!(key_id, ALIAS);
        Ok((ARN.to_string(), seal(ARN, plaintext)))
    }

    async fn decrypt(
        &self,
        _sealed_by: &str,
        ciphertext: &[u8],
    ) -> Result<(String, Vec<u8>), String> {
        self.calls.lock().unwrap().push("Decrypt");
        if let Some(e) = self.fail {
            return Err(e.into());
        }
        // The key is whatever the blob says, never what the caller passed.
        let split = ciphertext.iter().position(|b| *b == 0).ok_or("not ours")?;
        let arn = std::str::from_utf8(&ciphertext[..split]).map_err(|e| e.to_string())?;
        let plain = ciphertext[split + 1..]
            .iter()
            .map(|b| b ^ pad(arn))
            .collect();
        Ok((self.opened_by.unwrap_or(arn).to_string(), plain))
    }
}

impl FakeKms {
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct FakeStore {
    row: Mutex<Option<Vec<u8>>>,
    /// A row another process writes between our insert and read-back.
    raced: Option<Vec<u8>>,
    variable_fails: bool,
    verified: Mutex<u32>,
    reads: Mutex<u32>,
}

impl MasterKeyStore for FakeStore {
    async fn read(&self) -> Result<Option<Vec<u8>>, String> {
        *self.reads.lock().unwrap() += 1;
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

const A: [u8; 32] = [3; 32];
const B: [u8; 32] = [4; 32];

/// A row sealed by `arn`, as an earlier boot left it.
fn row_by(arn: &str, key: [u8; 32]) -> Vec<u8> {
    StoredMasterKey {
        sealed_by: arn.to_string(),
        ciphertext: seal(arn, &key),
    }
    .to_bytes()
}

fn with_row(row: Vec<u8>) -> FakeStore {
    FakeStore {
        row: Mutex::new(Some(row)),
        ..Default::default()
    }
}

fn stored(key: [u8; 32], sealed_by: &str) -> Result<Resolved, String> {
    Ok(Resolved::Stored {
        key,
        sealed_by: sealed_by.to_string(),
    })
}

// ── Row present ───────────────────────────────────────────────────────────

#[tokio::test]
async fn a_present_row_is_opened_with_no_key_id_and_needs_no_variable() {
    // Sealed by a key no alias in this namespace names: the row decides, not
    // the namespace.
    let other = "arn:aws:kms:us-west-2:1:key/sealed-elsewhere";
    let kms = FakeKms {
        lookup: Err("DescribeKey must not be called when the row is there"),
        ..Default::default()
    };
    let store = with_row(row_by(other, A));
    assert_eq!(
        resolve(&kms, &store, Some(NS), true, None).await,
        stored(A, other)
    );
    assert_eq!(
        resolve(&kms, &store, Some(NS), true, Some(A)).await,
        stored(A, other)
    );
    // `oxy migrate` opens it the same way.
    assert_eq!(
        resolve(&kms, &store, Some(NS), false, None).await,
        stored(A, other)
    );
    assert_eq!(kms.calls(), ["Decrypt"; 3]);
    assert_eq!(
        *store.verified.lock().unwrap(),
        0,
        "a present row is not re-checked against data"
    );
}

#[tokio::test]
async fn a_variable_that_differs_from_the_row_is_refused_naming_both() {
    let store = with_row(row_by(ARN, A));
    let err = resolve(&FakeKms::default(), &store, Some(NS), true, Some(B))
        .await
        .unwrap_err();
    assert!(
        err.contains("OXY_ENCRYPTION_KEY is set and is not the master key"),
        "{err}"
    );
    assert!(err.contains("master_key") && err.contains(ARN), "{err}");
}

#[tokio::test]
async fn kms_opening_the_row_with_another_key_than_it_names_is_refused() {
    let kms = FakeKms {
        opened_by: Some("arn:aws:kms:us-west-2:1:key/another"),
        ..Default::default()
    };
    let err = resolve(&kms, &with_row(row_by(ARN, A)), Some(NS), true, None)
        .await
        .unwrap_err();
    assert!(
        err.contains(
            "with key arn:aws:kms:us-west-2:1:key/another, but the row says it was sealed by"
        ),
        "{err}"
    );
    assert!(err.contains(ARN), "{err}");
}

// ── Row absent, in Kubernetes, the alias exists ─────────────────────────────

#[tokio::test]
async fn no_row_seals_the_variable_with_the_namespace_alias_after_the_data_check() {
    let kms = FakeKms::default();
    let store = FakeStore::default();
    assert_eq!(
        resolve(&kms, &store, Some(NS), true, Some(A)).await,
        stored(A, ARN)
    );
    assert_eq!(*store.verified.lock().unwrap(), 1);
    assert_eq!(kms.calls(), ["DescribeKey", "Encrypt", "Decrypt"]);
    let row = StoredMasterKey::from_bytes(&store.row.lock().unwrap().clone().unwrap()).unwrap();
    assert_eq!(
        row.sealed_by, ARN,
        "the row records the key's ARN, not the alias"
    );
    // The next boot needs no variable, and does not look the alias up.
    let kms = FakeKms::default();
    assert_eq!(
        resolve(&kms, &store, Some(NS), true, None).await,
        stored(A, ARN)
    );
    assert_eq!(kms.calls(), ["Decrypt"]);
}

#[tokio::test]
async fn no_row_and_no_variable_is_refused_and_nothing_is_made() {
    let store = FakeStore::default();
    let err = resolve(&FakeKms::default(), &store, Some(NS), true, None)
        .await
        .unwrap_err();
    assert!(err.contains(ALIAS), "{err}");
    assert!(err.contains("needs OXY_ENCRYPTION_KEY"), "{err}");
    assert!(store.row.lock().unwrap().is_none());
}

#[tokio::test]
async fn a_variable_that_fails_the_data_check_is_not_recorded() {
    let store = FakeStore {
        variable_fails: true,
        ..Default::default()
    };
    let err = resolve(&FakeKms::default(), &store, Some(NS), true, Some(B))
        .await
        .unwrap_err();
    assert!(err.contains("opens none"), "{err}");
    assert!(store.row.lock().unwrap().is_none());
}

#[tokio::test]
async fn losing_the_race_to_a_different_key_is_refused() {
    let store = FakeStore {
        raced: Some(row_by(ARN, A)),
        ..Default::default()
    };
    let err = resolve(&FakeKms::default(), &store, Some(NS), true, Some(B))
        .await
        .unwrap_err();
    assert!(
        err.contains("Another process recorded a different master key"),
        "{err}"
    );
    // Losing it to the same key is fine.
    let store = FakeStore {
        raced: Some(row_by(ARN, A)),
        ..Default::default()
    };
    assert_eq!(
        resolve(&FakeKms::default(), &store, Some(NS), true, Some(A)).await,
        stored(A, ARN)
    );
}

#[tokio::test]
async fn oxy_migrate_never_seals_a_new_row() {
    let kms = FakeKms::default();
    let store = FakeStore::default();
    assert_eq!(
        resolve(&kms, &store, Some(NS), false, Some(A)).await,
        Ok(Resolved::NotSealed { warning: None })
    );
    assert!(kms.calls().is_empty());
    assert!(store.row.lock().unwrap().is_none());
}

// ── Row absent, in Kubernetes, the alias does not exist ─────────────────────

#[tokio::test]
async fn a_missing_alias_keeps_the_variable_with_a_warning() {
    let kms = FakeKms {
        lookup: Ok(KeyLookup::NotFound),
        ..Default::default()
    };
    let store = FakeStore::default();
    for variable in [Some(A), None] {
        assert_eq!(
            resolve(&kms, &store, Some(NS), true, variable).await,
            Ok(Resolved::NotSealed {
                warning: Some(
                    "the master key is not sealed by KMS because \
                     alias/oxy-test-oxy-master-key does not exist; using OXY_ENCRYPTION_KEY \
                     as before"
                        .into()
                ),
            })
        );
    }
    assert_eq!(kms.calls(), ["DescribeKey"; 2], "nothing sealed");
    assert!(store.row.lock().unwrap().is_none());
    assert_eq!(*store.verified.lock().unwrap(), 0);
}

#[tokio::test]
async fn no_aws_in_the_process_keeps_the_variable_with_a_warning() {
    let kms = FakeKms {
        lookup: Ok(KeyLookup::NoAws("no AWS credentials".into())),
        ..Default::default()
    };
    let Ok(Resolved::NotSealed { warning: Some(w) }) =
        resolve(&kms, &FakeStore::default(), Some(NS), true, Some(A)).await
    else {
        panic!("expected the variable");
    };
    assert!(w.contains(ALIAS) && w.contains("no AWS credentials"), "{w}");
}

#[test]
fn the_warning_is_logged_once() {
    let flag = AtomicBool::new(false);
    assert!(warn_once(&flag, Some("w")));
    assert!(!warn_once(&flag, Some("w")));
}

// ── Row absent, in Kubernetes, KMS refuses ───────────────────────────────────

#[tokio::test]
async fn kms_failure_is_refused_naming_the_key_and_the_role_with_no_fallback() {
    // DescribeKey denied: whether the alias exists is unknown.
    let denied_lookup = FakeKms {
        lookup: Err("AccessDeniedException: not authorized"),
        ..Default::default()
    };
    let err = resolve(
        &denied_lookup,
        &FakeStore::default(),
        Some(NS),
        true,
        Some(A),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains(&format!("KMS DescribeKey on {ALIAS} failed")),
        "{err}"
    );
    assert!(err.contains("EKS Pod Identity"), "{err}");

    // The alias exists and Encrypt is denied; a present row and Decrypt denied.
    let denied = FakeKms {
        fail: Some("AccessDeniedException: not authorized"),
        ..Default::default()
    };
    for (store, call, key) in [
        (FakeStore::default(), "Encrypt", ALIAS),
        (with_row(row_by(ARN, A)), "Decrypt", ARN),
    ] {
        let err = resolve(&denied, &store, Some(NS), true, Some(A))
            .await
            .unwrap_err();
        assert!(
            err.contains(&format!("KMS {call} on {key} failed")),
            "{err}"
        );
        assert!(err.contains("AccessDeniedException"), "{err}");
        assert!(err.contains("EKS Pod Identity"), "{err}");
        assert!(err.contains("Refusing to start"), "{err}");
    }
}

// ── Not in Kubernetes ───────────────────────────────────────────────────────

#[tokio::test]
async fn outside_kubernetes_nothing_is_read_or_asked() {
    let kms = FakeKms::default();
    for store in [FakeStore::default(), with_row(row_by(ARN, A))] {
        for may_seal in [true, false] {
            assert_eq!(
                resolve(&kms, &store, None, may_seal, Some(A)).await,
                Ok(Resolved::NotSealed { warning: None })
            );
        }
        assert_eq!(*store.reads.lock().unwrap(), 0);
    }
    assert!(kms.calls().is_empty());
}

#[test]
fn the_namespace_comes_from_the_service_account_mount() {
    let path = std::env::temp_dir().join(format!("oxy-master-key-ns-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    assert_eq!(read_namespace(&path), None, "no file: not in Kubernetes");
    std::fs::write(&path, "oxy-staging\n").unwrap();
    assert_eq!(read_namespace(&path).as_deref(), Some("oxy-staging"));
    std::fs::write(&path, "  ").unwrap();
    assert_eq!(read_namespace(&path), None);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(alias_for("oxy-staging"), "alias/oxy-staging-oxy-master-key");
    assert_eq!(alias_for("oxy"), "alias/oxy-oxy-master-key");
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
