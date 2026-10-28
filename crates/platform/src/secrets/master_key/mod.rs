//! The master key in Postgres, sealed by AWS KMS.
//!
//! With `OXY_ENCRYPTION_KMS_KEY_ID` set (a KMS key id, ARN or alias), the
//! master key is `server_keys` row [`MASTER_KEY_ROW`]: the 32 key bytes sealed
//! by that KMS key, with the id of the key that sealed them. Every process of
//! a deployment shares the database, so they agree on the key by construction,
//! and the key never sits in an environment blob or on disk. Read once at
//! boot ([`load_at_boot`]) and handed out from then on by
//! [`super::get_encryption_key`].
//!
//! ## Boot
//!
//! - **Row present** → KMS `Decrypt` → that is the master key. If
//!   `OXY_ENCRYPTION_KEY` is also set and is a different key, refuse to start,
//!   naming both: one of them is wrong, and guessing which seals data under the
//!   other.
//! - **Row absent** → `OXY_ENCRYPTION_KEY` is required. It must pass
//!   [`super::key_check`] (it opens what the database already holds), then it
//!   is sealed with KMS `Encrypt` and inserted if absent, the row is read back
//!   and opened, and the key it holds is the one used. A process that loses the
//!   race to another holding a different key refuses, as for the check value.
//! - **KMS unreachable or access denied** → refuse to start, naming the key id
//!   and the role. Never fall back to the variable, a file or a new key.
//!
//! Without the setting nothing here runs: the variable, then the key file for
//! `OXY_ROLE=all`, as before.
//!
//! Rotation is not implemented. It would decrypt this row and seal a new key
//! here, and re-seal every value the sources in `key_check` list.

mod aws;
mod row;
mod store;
#[cfg(test)]
mod tests;

use std::future::Future;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::encryption::{
    ENCRYPTION_KEY_VAR, KMS_KEY_ID_VAR, MasterKeySource, install_master_key, installed_master_key,
    kms_key_id, parse_encryption_key,
};
pub use aws::AwsKms;
pub use row::StoredMasterKey;
pub use store::PostgresStore;

/// `server_keys.name` of the KMS-sealed master key.
pub const MASTER_KEY_ROW: &str = "master_key";

/// The two KMS calls this needs. A trait so the boot rule is testable without
/// AWS; [`AwsKms`] is the real one.
pub trait Kms {
    /// Seal `plaintext` under `key_id`: the ciphertext and the ARN of the key
    /// that sealed it.
    fn encrypt(
        &self,
        key_id: &str,
        plaintext: &[u8],
    ) -> impl Future<Output = Result<(String, Vec<u8>), String>> + Send;
    fn decrypt(
        &self,
        key_id: &str,
        ciphertext: &[u8],
    ) -> impl Future<Output = Result<Vec<u8>, String>> + Send;
}

/// Where the row lives, and the check the variable must pass before it is
/// recorded. A trait so the boot rule is testable without Postgres;
/// [`PostgresStore`] is the real one.
pub trait MasterKeyStore {
    fn read(&self) -> impl Future<Output = Result<Option<Vec<u8>>, String>> + Send;
    fn insert_if_absent(&self, row: &[u8]) -> impl Future<Output = Result<(), String>> + Send;
    /// [`key_check::verify_key`] for the variable's key.
    fn verify_variable(&self, key: &[u8; 32]) -> impl Future<Output = Result<(), String>> + Send;
}

fn kms_refusal(what: &str, key_id: &str, e: &str) -> String {
    format!(
        "Could not {what} the master key with KMS key {key_id} ({KMS_KEY_ID_VAR}): {e}\n\n\
         This process's AWS credentials come from the IAM role bound to its Kubernetes service \
         account through EKS Pod Identity; that role needs kms:Encrypt, kms:Decrypt and \
         kms:DescribeKey on this key, and an alias must exist in the region the call went to \
         (AWS_REGION, else OXY_DATABASE_REGION). \
         Refusing to start: without the master key every stored secret is unreadable, and no \
         other key will be used in its place."
    )
}

fn row_refusal(e: impl std::fmt::Display) -> String {
    format!(
        "{e}. Refusing to start rather than use another key; the row is written by the first \
         boot that has {KMS_KEY_ID_VAR} set."
    )
}

fn as_key(bytes: Vec<u8>, key_id: &str) -> Result<[u8; 32], String> {
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
        row_refusal(format_args!(
            "server_keys row `{MASTER_KEY_ROW}` opened with KMS key {key_id} to {} bytes, \
             not the 32 of a master key",
            bytes.len()
        ))
    })
}

async fn open_row<K: Kms>(kms: &K, key_id: &str, row: &[u8]) -> Result<[u8; 32], String> {
    let stored = StoredMasterKey::from_bytes(row).map_err(row_refusal)?;
    let plain = kms
        .decrypt(key_id, &stored.ciphertext)
        .await
        .map_err(|e| kms_refusal("decrypt", key_id, &e))?;
    as_key(plain, key_id)
}

/// The boot rule. `variable` is `OXY_ENCRYPTION_KEY`, decoded, when set.
pub async fn resolve<K: Kms, S: MasterKeyStore>(
    kms: &K,
    store: &S,
    key_id: &str,
    variable: Option<[u8; 32]>,
) -> Result<[u8; 32], String> {
    if let Some(row) = store.read().await? {
        let key = open_row(kms, key_id, &row).await?;
        if let Some(variable) = variable
            && variable != key
        {
            return Err(format!(
                "{ENCRYPTION_KEY_VAR} is set and is not the master key stored in server_keys row \
                 `{MASTER_KEY_ROW}` (opened with KMS key {key_id}). One of them is wrong. If the \
                 row was written by this deployment, remove {ENCRYPTION_KEY_VAR} or set it to \
                 the key the row holds; if the variable is the key this deployment's data was \
                 sealed with, the row is not this deployment's. Refusing to start rather than \
                 seal data under either."
            ));
        }
        return Ok(key);
    }
    let Some(variable) = variable else {
        return Err(format!(
            "{KMS_KEY_ID_VAR} is set and server_keys has no `{MASTER_KEY_ROW}` row yet, so the \
             first boot needs {ENCRYPTION_KEY_VAR} to seal into it: set it to this deployment's \
             current key for one deploy. Refusing to start rather than make a new key, which \
             could not open the data already here."
        ));
    };
    seal_variable(kms, store, key_id, &variable).await
}

/// No row yet: seal the variable into one, after it passes the data check,
/// and use whichever row ends up stored.
async fn seal_variable<K: Kms, S: MasterKeyStore>(
    kms: &K,
    store: &S,
    key_id: &str,
    variable: &[u8; 32],
) -> Result<[u8; 32], String> {
    store.verify_variable(variable).await?;
    let (sealed_by, ciphertext) = kms
        .encrypt(key_id, variable)
        .await
        .map_err(|e| kms_refusal("encrypt", key_id, &e))?;
    store
        .insert_if_absent(
            &StoredMasterKey {
                sealed_by,
                ciphertext,
            }
            .to_bytes(),
        )
        .await?;
    // Read back: another process booting at the same moment may have written
    // first, and its row is the one every process will use.
    let row = store.read().await?.ok_or_else(|| {
        row_refusal(format_args!(
            "server_keys row `{MASTER_KEY_ROW}` vanished after it was written"
        ))
    })?;
    let key = open_row(kms, key_id, &row).await?;
    if key != *variable {
        return Err(format!(
            "Another process recorded a different master key in server_keys row \
             `{MASTER_KEY_ROW}` while this one was booting with {ENCRYPTION_KEY_VAR}. The \
             processes of this deployment do not hold the same {ENCRYPTION_KEY_VAR}; give every \
             one the same value."
        ));
    }
    tracing::info!(
        kms_key_id = key_id,
        "master key sealed by KMS and stored in server_keys"
    );
    Ok(key)
}
/// [`resolve`] with `kms`, then install the key for the life of the process.
/// Separate from [`load_at_boot`] so a test can pass a fake KMS.
pub async fn load_with<K: Kms, C: ConnectionTrait>(
    kms: &K,
    db: &C,
    key_id: &str,
) -> Result<(), String> {
    let raw = std::env::var(ENCRYPTION_KEY_VAR)
        .ok()
        .filter(|v| !v.is_empty());
    let variable = raw
        .as_deref()
        .map(|v| {
            parse_encryption_key(v)
                .map_err(|e| format!("{ENCRYPTION_KEY_VAR}: set but unusable ({e})."))
        })
        .transpose()?;
    let key = resolve(kms, &PostgresStore { db }, key_id, variable).await?;
    install_master_key(
        key,
        MasterKeySource::Kms {
            key_id: key_id.to_string(),
        },
    )?;
    // `oxy_auth::session_key` mixes the variable's raw text in while it is set,
    // and the canonical base64 of this key once it is gone. Differing text
    // means removing the variable signs everyone out once.
    if let Some(raw) = raw
        && raw != BASE64.encode(key)
    {
        tracing::warn!(
            "{ENCRYPTION_KEY_VAR} is not the canonical base64 of the master key (whitespace or \
             padding differs); removing it will end every browser session once"
        );
    }
    Ok(())
}

/// Load the master key from `server_keys` when `OXY_ENCRYPTION_KMS_KEY_ID` is
/// set; nothing otherwise. Idempotent, and a no-op while `server_keys` does not
/// exist yet (a fresh database before migrations), so entry points call it
/// both before migrations (a migration may seal a value) and after.
pub async fn load_at_boot<C: ConnectionTrait>(db: &C) -> Result<(), String> {
    let Some(key_id) = kms_key_id() else {
        return Ok(());
    };
    if installed_master_key().is_some() {
        return Ok(());
    }
    let exists = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT to_regclass('server_keys') IS NOT NULL AS present",
        ))
        .await
        .map_err(store::db_error)?
        .map(|r| r.try_get::<bool>("", "present"))
        .transpose()
        .map_err(store::db_error)?
        .unwrap_or(false);
    if !exists {
        return Ok(());
    }
    load_with(&AwsKms::from_env(&key_id).await?, db, &key_id).await
}
