//! The sealed values already in the database, tried on a key: what the first
//! boot looks at before it records a check value.

use base64::{Engine as _, engine::general_purpose};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::db_error;
use crate::secrets::envelope;

/// How many values of each kind the first boot tries, newest first. Opening one
/// is microseconds; the tables are small.
pub const SAMPLE_PER_KIND: u32 = 500;
/// How many ids of values that do not open are kept per kind, for the log.
const FAILED_IDS_KEPT: usize = 20;

/// How a sealed column stores its bytes.
#[derive(Clone, Copy, Debug)]
pub(super) enum Encoding {
    /// `nonce || ciphertext` as `BYTEA` ([`envelope::seal`]).
    Envelope,
    /// The same bytes, base64, as `TEXT` (the legacy `_var` secret manager).
    Base64Envelope,
    /// One version byte (`0x01`), then the envelope (`oxy_cameras::secrets`).
    VersionedEnvelope,
}

/// One kind of sealed value to try the key on.
struct Source {
    /// Named in the refusal and in the log.
    what: &'static str,
    table: &'static str,
    /// Selects `id` (as text) and the value `v`, newest first, ending in
    /// `LIMIT` (the bound is appended).
    sql: &'static str,
    encoding: Encoding,
}

/// Every table in this database that holds values sealed with the master key.
/// A table that does not exist (its migrator never ran here) is skipped.
const SOURCES: &[Source] = &[
    Source {
        what: "org secrets (org_secrets.ciphertext)",
        table: "org_secrets",
        sql: "SELECT id::text AS id, ciphertext AS v FROM org_secrets WHERE key_version = 1 \
              ORDER BY updated_at DESC LIMIT",
        encoding: Encoding::Envelope,
    },
    Source {
        what: "workspace secrets (secrets.encrypted_value)",
        table: "secrets",
        sql: "SELECT id::text AS id, encrypted_value AS v FROM secrets \
              ORDER BY updated_at DESC LIMIT",
        encoding: Encoding::Base64Envelope,
    },
    Source {
        what: "Airhouse service-account bearers (airhouse_tenants.bearer_ciphertext)",
        table: "airhouse_tenants",
        sql: "SELECT id::text AS id, bearer_ciphertext AS v FROM airhouse_tenants \
              WHERE bearer_ciphertext IS NOT NULL \
              ORDER BY COALESCE(sa_rotated_at, sa_created_at, created_at) DESC LIMIT",
        encoding: Encoding::Envelope,
    },
    Source {
        what: "OLTP owner passwords (oltp_tenants.owner_password_ciphertext)",
        table: "oltp_tenants",
        sql: "SELECT id::text AS id, owner_password_ciphertext AS v FROM oltp_tenants \
              WHERE owner_password_ciphertext IS NOT NULL ORDER BY updated_at DESC LIMIT",
        encoding: Encoding::Envelope,
    },
    Source {
        what: "camera device secrets (device_registry.device_secret)",
        table: "device_registry",
        // Rows the sealing migration has not reached are plaintext; only a
        // row with the version byte is a sealed value.
        sql: "SELECT device_id::text AS id, device_secret AS v FROM device_registry \
              WHERE substring(device_secret from 1 for 1) = '\\x01'::bytea \
              ORDER BY manufactured_at DESC LIMIT",
        encoding: Encoding::VersionedEnvelope,
    },
];

/// The existing sealed values of one kind the key was tried on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub what: &'static str,
    pub opened: u32,
    pub tried: u32,
    /// Ids of rows that did not open, up to [`FAILED_IDS_KEPT`].
    pub failed_ids: Vec<String>,
}

pub(super) fn opens(key: &[u8; 32], encoding: Encoding, value: &[u8]) -> bool {
    let blob = match encoding {
        Encoding::Envelope => value.to_vec(),
        Encoding::Base64Envelope => match general_purpose::STANDARD.decode(value) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        },
        Encoding::VersionedEnvelope => match value.split_first() {
            Some((1, rest)) => rest.to_vec(),
            _ => return false,
        },
    };
    envelope::open_with_key(key, &blob).is_ok()
}

async fn table_exists<C: ConnectionTrait>(db: &C, table: &str) -> Result<bool, String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT to_regclass($1) IS NOT NULL AS present",
            [table.into()],
        ))
        .await
        .map_err(|e| db_error(table, e))?;
    match row {
        Some(r) => r
            .try_get::<bool>("", "present")
            .map_err(|e| db_error(table, e)),
        None => Ok(false),
    }
}

async fn sample_one<C: ConnectionTrait>(
    db: &C,
    key: &[u8; 32],
    source: &Source,
) -> Result<Sample, String> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("{} {SAMPLE_PER_KIND}", source.sql),
        ))
        .await
        .map_err(|e| db_error(source.table, e))?;
    let mut sample = Sample {
        what: source.what,
        opened: 0,
        tried: 0,
        failed_ids: Vec::new(),
    };
    for row in rows {
        let id: String = row
            .try_get("", "id")
            .map_err(|e| db_error(source.table, e))?;
        let value = match source.encoding {
            Encoding::Base64Envelope => row.try_get::<String>("", "v").map(String::into_bytes),
            _ => row.try_get::<Vec<u8>>("", "v"),
        }
        .map_err(|e| db_error(source.table, e))?;
        sample.tried += 1;
        if opens(key, source.encoding, &value) {
            sample.opened += 1;
        } else if sample.failed_ids.len() < FAILED_IDS_KEPT {
            sample.failed_ids.push(id);
        }
    }
    Ok(sample)
}

/// Up to [`SAMPLE_PER_KIND`] values of each kind this database has, newest
/// first, each tried on `key`. A kind with no values is left out.
pub async fn sample_existing<C: ConnectionTrait>(
    db: &C,
    key: &[u8; 32],
) -> Result<Vec<Sample>, String> {
    let mut samples = Vec::new();
    for source in SOURCES {
        if !table_exists(db, source.table).await? {
            continue;
        }
        let sample = sample_one(db, key, source).await?;
        if sample.tried > 0 {
            samples.push(sample);
        }
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_encoding_opens_only_under_its_own_key() {
        let (key, other) = ([7u8; 32], [8u8; 32]);
        let blob = envelope::seal_with_key(&key, b"value").unwrap();
        let b64 = general_purpose::STANDARD.encode(&blob).into_bytes();
        let mut versioned = vec![1u8];
        versioned.extend_from_slice(&blob);
        for (encoding, value) in [
            (Encoding::Envelope, &blob),
            (Encoding::Base64Envelope, &b64),
            (Encoding::VersionedEnvelope, &versioned),
        ] {
            assert!(opens(&key, encoding, value), "{encoding:?}");
            assert!(!opens(&other, encoding, value), "{encoding:?}");
        }
        // Not base64, and not the version byte: not a value that opens.
        assert!(!opens(&key, Encoding::Base64Envelope, b"not base64!!"));
        assert!(!opens(&key, Encoding::VersionedEnvelope, &blob));
    }
}
