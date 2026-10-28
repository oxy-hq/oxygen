//! What the boot check tells an operator: the refusals, and a fingerprint that
//! names a key without revealing it.

use sha2::{Digest, Sha256};

use super::sample::Sample;
use super::{CHECK_ROW, Decision};

/// Eight hex digits that name a key without revealing it, for comparing the
/// processes of a deployment: they must all print the same one. A hash of the
/// key under its own domain, cut to four bytes.
pub fn fingerprint(key: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"oxy-encryption-key-fingerprint:v1\0");
    hasher.update(key);
    hasher.finalize()[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One line per kind: how many of the values tried opened.
pub(super) fn counts(samples: &[Sample]) -> String {
    samples
        .iter()
        .map(|s| {
            format!(
                "\n  {}: opens {} of the newest {}",
                s.what, s.opened, s.tried
            )
        })
        .collect()
}

const RESTORE: &str = "Do not generate a new key: that cannot open the existing data either. \
     Restore the previous value of OXY_ENCRYPTION_KEY (every process of a deployment must hold \
     the same one; for `oxy start`, the key file in its state directory) and start again. If \
     that key is lost, see internal-docs/backup-recovery.md, \"OXY_ENCRYPTION_KEY is lost\".";

const UNREADABLE: &str = "It is not the key this deployment's secrets were sealed with. \
     Started with it, every stored secret (org and workspace secrets, Airhouse and OLTP \
     credentials, camera device secrets) would fail to open while this process reported \
     healthy.";

/// The message to refuse to start with, or `None` for a decision that starts.
pub(super) fn refusal(decision: &Decision, source: &str, key: &[u8; 32]) -> Option<String> {
    let fp = fingerprint(key);
    match decision {
        Decision::RefuseCheckRow => Some(format!(
            "The encryption key from {source} (fingerprint {fp}) does not open this database's \
             encryption check value (server_keys row `{CHECK_ROW}`).\n\n{UNREADABLE}\n\n{RESTORE}"
        )),
        Decision::RefuseExistingData { samples } => Some(format!(
            "The encryption key from {source} (fingerprint {fp}) opens none of the sealed values \
             already in this database:{}\n\
             There is no encryption check value yet, so none was written under this key.\n\n\
             {UNREADABLE}\n\n{RESTORE}",
            counts(samples)
        )),
        Decision::Proceed | Decision::WriteCheckRow => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_names_a_key_without_revealing_it() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        assert_eq!(fingerprint(&a).len(), 8);
        assert_eq!(fingerprint(&a), fingerprint(&a));
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn each_refusal_names_the_key_and_says_not_to_generate_one() {
        let key = [3u8; 32];
        let fp = fingerprint(&key);
        let msg = refusal(&Decision::RefuseCheckRow, "OXY_ENCRYPTION_KEY", &key).unwrap();
        assert!(
            msg.contains(&format!("from OXY_ENCRYPTION_KEY (fingerprint {fp})")),
            "{msg}"
        );
        assert!(msg.contains("encryption_check"), "{msg}");
        assert!(msg.contains("Do not generate a new key"), "{msg}");
        assert!(msg.contains("Restore the previous value"), "{msg}");

        let samples = vec![Sample {
            what: "org secrets (org_secrets.ciphertext)",
            opened: 0,
            tried: 4,
            failed_ids: vec![],
        }];
        let msg = refusal(
            &Decision::RefuseExistingData { samples },
            "OXY_ENCRYPTION_KEY",
            &key,
        )
        .unwrap();
        assert!(msg.contains("opens none of the sealed values"), "{msg}");
        assert!(
            msg.contains("org_secrets.ciphertext): opens 0 of the newest 4"),
            "{msg}"
        );
        assert!(msg.contains("none was written"), "{msg}");

        assert_eq!(refusal(&Decision::WriteCheckRow, "x", &key), None);
    }
}
