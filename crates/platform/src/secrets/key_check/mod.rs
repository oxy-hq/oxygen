//! Boot-time refusal: a process must not start with an encryption key that
//! cannot open what this database's secrets were sealed with.
//!
//! `shared_state_check` (in `oxy-app`) makes sure `OXY_ENCRYPTION_KEY` is
//! *set* on every fleet pod. Nothing checked that it is the *right* key. A
//! wrong one is silent: each sealed value fails to open where it is read
//! (measured in staging, 2026-10-28: a worker logged `Decryption failed:
//! aead::Error` 88 times compiling one workspace and fell back to default
//! ClickHouse settings), while every pod reports healthy.
//!
//! ## The check value
//!
//! `server_keys` row [`CHECK_ROW`] holds [`CHECK_PLAINTEXT`], a fixed and
//! non-secret string, sealed with the deployment's key. Every boot opens it.
//! It does not open → the key is not the one this deployment seals with →
//! refuse to start. The row is written by whichever process boots first
//! (`INSERT … ON CONFLICT DO NOTHING`, then read back, as
//! `oxy_auth::session_key` does), so two processes booting together end up
//! checking the same row, and one holding a different key refuses.
//!
//! ## The first boot of a deployment that has no check value yet
//!
//! Every deployment that predates this check has sealed data and no row. A
//! process must not write the row under a key that opens none of it: every
//! later boot with the right key would be refused. So before writing it, the
//! process tries its key on the sealed values already there: up to
//! [`sample::SAMPLE_PER_KIND`] of each kind, newest first.
//!
//! - no sealed value anywhere (a new deployment, `oxy start` on a fresh local
//!   database) → write the row;
//! - none of them opens → the key is wrong: refuse, and write nothing;
//! - at least one opens → write the row and start. Values that do not open are
//!   logged at `error`, with their count and the ids of the rows, per table.
//!
//! The third case starts rather than refuses because a real deployment has
//! such rows: in staging (2026-10-28) the `CLICKHOUSE_*` `_var` secrets of one
//! workspace fail under the key every staging pod holds (their fingerprints
//! match) while the rest of staging works; they were sealed elsewhere and
//! copied in. Refusing would take the whole deployment down over values that
//! are already unreadable. The cost: on a fleet whose pods disagree on the key,
//! a pod with the wrong key opens the values it sealed itself and could record
//! its key first. Every process logs its key's [`fingerprint`]; across one
//! deployment they must all be the same, and the error log names the rows a
//! key does not open.
//!
//! Rotation (open with the previous key, seal again with the new one, rewrite
//! this row) is not implemented; this module is where it would go.

mod message;
mod sample;

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::encryption::{
    ENCRYPTION_KEY_VAR, encryption_key_source, get_encryption_key, parse_encryption_key,
};
use super::envelope;
pub use message::fingerprint;
pub use sample::Sample;

/// `server_keys.name` of the check value.
pub const CHECK_ROW: &str = "encryption_check";
/// What the check value seals. Not a secret: what matters is only whether it
/// opens.
pub const CHECK_PLAINTEXT: &[u8] = b"oxy-encryption-key-check:v1";

/// What the database told us, reduced to what the decision needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckRow {
    Missing,
    Opens,
    DoesNotOpen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The check value opens.
    Proceed,
    /// No check value yet, and the key opens the existing data, or some of it,
    /// or there is none.
    WriteCheckRow,
    /// The check value does not open.
    RefuseCheckRow,
    /// No check value yet, and none of the existing sealed values opens.
    RefuseExistingData { samples: Vec<Sample> },
}

/// The rule, separate from the database. `samples` is only consulted when
/// there is no check row; see the module comment for why some values that do
/// not open are tolerated.
pub fn decide(row: CheckRow, samples: &[Sample]) -> Decision {
    let opened: u32 = samples.iter().map(|s| s.opened).sum();
    let tried: u32 = samples.iter().map(|s| s.tried).sum();
    match row {
        CheckRow::Opens => Decision::Proceed,
        CheckRow::DoesNotOpen => Decision::RefuseCheckRow,
        CheckRow::Missing if tried > 0 && opened == 0 => Decision::RefuseExistingData {
            samples: samples.to_vec(),
        },
        CheckRow::Missing => Decision::WriteCheckRow,
    }
}

fn db_error(what: &str, e: impl std::fmt::Display) -> String {
    format!(
        "Could not check {ENCRYPTION_KEY_VAR} against this database ({what}: {e}). \
         The check runs after migrations and needs the server_keys table."
    )
}

async fn read_check_row<C: ConnectionTrait>(db: &C) -> Result<Option<Vec<u8>>, String> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT secret FROM server_keys WHERE name = $1",
            [CHECK_ROW.into()],
        ))
        .await
        .map_err(|e| db_error("read server_keys", e))?;
    row.map(|r| r.try_get::<Vec<u8>>("", "secret"))
        .transpose()
        .map_err(|e| db_error("read server_keys", e))
}

fn check(key: &[u8; 32], sealed: &[u8]) -> CheckRow {
    if envelope::open_with_key(key, sealed).is_ok_and(|p| p == CHECK_PLAINTEXT) {
        CheckRow::Opens
    } else {
        CheckRow::DoesNotOpen
    }
}

/// Insert the check value if absent, then read back what is there: a process
/// booting at the same moment may have written first.
async fn write_check_row<C: ConnectionTrait>(db: &C, key: &[u8; 32]) -> Result<CheckRow, String> {
    let sealed = envelope::seal_with_key(key, CHECK_PLAINTEXT)
        .map_err(|e| format!("Could not seal the encryption check value: {e}"))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO server_keys (name, secret) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING",
        [CHECK_ROW.into(), sealed.into()],
    ))
    .await
    .map_err(|e| db_error("write server_keys", e))?;
    let stored = read_check_row(db)
        .await?
        .ok_or_else(|| db_error("read server_keys", "the row just written is missing"))?;
    Ok(check(key, &stored))
}

/// The values the key does not open, where it opens others: logged, not
/// refused (see the module comment).
fn log_unopened(samples: &[Sample], key: &[u8; 32]) {
    for sample in samples.iter().filter(|s| s.opened < s.tried) {
        tracing::error!(
            sealed_value = sample.what,
            failed = sample.tried - sample.opened,
            tried = sample.tried,
            row_ids = %sample.failed_ids.join(","),
            key_fingerprint = %fingerprint(key),
            "sealed values that do not open with this deployment's encryption key: they were \
             sealed under another key and are unreadable; the key opens other values, so the \
             process starts"
        );
    }
}

/// Open the check value with `key`, writing it first when the rule allows.
/// `source` names where the key came from. `Err` is the message to refuse to
/// start with.
pub async fn verify_key<C: ConnectionTrait>(
    db: &C,
    key: &[u8; 32],
    source: &str,
) -> Result<(), String> {
    let refuse = |decision: &Decision| match message::refusal(decision, source, key) {
        Some(message) => Err(message),
        None => Ok(()),
    };

    if let Some(sealed) = read_check_row(db).await? {
        return refuse(&decide(check(key, &sealed), &[]));
    }

    let samples = sample::sample_existing(db, key).await?;
    let decision = decide(CheckRow::Missing, &samples);
    if decision != Decision::WriteCheckRow {
        return refuse(&decision);
    }
    log_unopened(&samples, key);
    let stored = write_check_row(db, key).await?;
    refuse(&decide(stored, &[]))?;
    tracing::info!(
        key_fingerprint = %fingerprint(key),
        sealed_values_tried = samples.iter().map(|s| s.tried).sum::<u32>(),
        "encryption check value written to server_keys"
    );
    Ok(())
}

/// [`verify_key`] with the key this process loaded, for the entry points
/// (`oxy serve`, `oxy worker`), after migrations.
pub async fn check_at_boot<C: ConnectionTrait>(db: &C) -> Result<(), String> {
    // A malformed variable panics in the loader. For a fleet role
    // `shared_state_check` already said so; for `OXY_ROLE=all` say it here.
    if let Ok(value) = std::env::var(ENCRYPTION_KEY_VAR) {
        parse_encryption_key(&value)
            .map_err(|e| format!("{ENCRYPTION_KEY_VAR}: set but unusable ({e})."))?;
    }
    let key = get_encryption_key();
    tracing::info!(key_fingerprint = %fingerprint(&key), "checking the encryption key");
    verify_key(db, &key, &encryption_key_source()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(opened: u32, tried: u32) -> Sample {
        Sample {
            what: "kind",
            opened,
            tried,
            failed_ids: vec![],
        }
    }

    #[test]
    fn no_row_and_no_sealed_data_writes_the_row() {
        assert_eq!(decide(CheckRow::Missing, &[]), Decision::WriteCheckRow);
    }

    #[test]
    fn no_row_and_sealed_data_that_opens_writes_the_row() {
        assert_eq!(
            decide(CheckRow::Missing, &[sample(3, 3), sample(40, 40)]),
            Decision::WriteCheckRow
        );
    }

    #[test]
    fn no_row_and_sealed_data_that_does_not_open_refuses() {
        let samples = [sample(0, 2), sample(0, 9)];
        assert_eq!(
            decide(CheckRow::Missing, &samples),
            Decision::RefuseExistingData {
                samples: samples.to_vec()
            }
        );
    }

    /// Staging has values sealed under another key next to ones that open:
    /// that starts, and the bad ones are logged.
    #[test]
    fn no_row_and_mixed_data_writes_the_row() {
        assert_eq!(
            decide(CheckRow::Missing, &[sample(1, 1), sample(28, 30)]),
            Decision::WriteCheckRow
        );
        assert_eq!(
            decide(CheckRow::Missing, &[sample(0, 4), sample(1, 1)]),
            Decision::WriteCheckRow
        );
    }

    #[test]
    fn a_row_that_opens_proceeds_whatever_the_data() {
        assert_eq!(decide(CheckRow::Opens, &[]), Decision::Proceed);
        assert_eq!(decide(CheckRow::Opens, &[sample(0, 5)]), Decision::Proceed);
    }

    #[test]
    fn a_row_that_does_not_open_refuses_whatever_the_data() {
        assert_eq!(decide(CheckRow::DoesNotOpen, &[]), Decision::RefuseCheckRow);
        assert_eq!(
            decide(CheckRow::DoesNotOpen, &[sample(5, 5)]),
            Decision::RefuseCheckRow
        );
    }
}
