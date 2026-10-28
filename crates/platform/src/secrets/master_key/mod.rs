//! The master key in Postgres, sealed by AWS KMS.
//!
//! The master key can be `server_keys` row [`MASTER_KEY_ROW`]: the 32 key
//! bytes sealed by a KMS key, with the ARN of the key that sealed them. Every
//! process of a deployment shares the database, so they agree on the key by
//! construction, and the key never sits in an environment blob or on disk.
//! Read once at boot ([`load_at_boot`]) and handed out from then on by
//! [`super::get_encryption_key`]. Nothing is configured: the row, and the
//! namespace a pod runs in, decide.
//!
//! ## Boot ([`resolve`])
//!
//! - **Row present** → KMS `Decrypt` with no key id: a symmetric KMS
//!   ciphertext carries the key that sealed it. The call goes to the region of
//!   the ARN stored in the row, the only region that can open it; the ARN is
//!   otherwise only for messages, and if KMS reports that a different key
//!   opened it, refuse. If `OXY_ENCRYPTION_KEY` is also set and is a different
//!   key, refuse to start, naming both: one of them is wrong, and guessing
//!   which seals data under the other.
//! - **Row absent** → the KMS key is `alias/<namespace>-oxy-master-key`
//!   ([`alias_for`]).
//!   - **The alias does not exist** (KMS `NotFoundException`), or this process
//!     has no AWS region or credentials to ask with → warn once, and use
//!     `OXY_ENCRYPTION_KEY`, then the key file for `OXY_ROLE=all`, as before
//!     the row existed. A cluster without the KMS key keeps working.
//!   - **It exists** → `OXY_ENCRYPTION_KEY` is required. It must pass
//!     [`super::key_check`] (it opens what the database already holds), then
//!     it is sealed with KMS `Encrypt` and inserted if absent, the row is read
//!     back and opened, and the key it holds is the one used. A process that
//!     loses the race to another holding a different key refuses, as for the
//!     check value.
//!   - **Access denied or KMS unreachable** → refuse to start, naming the
//!     alias and the role. Never fall back to the variable, a file or a new
//!     key.
//!
//! All of that is in Kubernetes only, where the pod's namespace is in
//! [`NAMESPACE_FILE`]. **Outside Kubernetes nothing here runs**, not even the
//! read of the row: the variable, then the key file for `OXY_ROLE=all`, as
//! before. Local `oxy start` is this case.
//!
//! `oxy migrate` ([`open_at_boot`]) opens a row that is there and never seals
//! a new one: sealing stays with `oxy serve` and `oxy worker`, which have the
//! variable to seal and compare.
//!
//! Rotation is not implemented. It would decrypt this row and seal a new key
//! here, and re-seal every value the sources in `key_check` list.

mod aws;
mod row;
mod store;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::encryption::{
    ENCRYPTION_KEY_VAR, MasterKeySource, install_master_key, installed_master_key,
    parse_encryption_key,
};
pub use aws::AwsKms;
pub use row::StoredMasterKey;
pub use store::PostgresStore;

/// `server_keys.name` of the KMS-sealed master key.
pub const MASTER_KEY_ROW: &str = "master_key";

/// The pod's namespace, mounted into every pod with its service account token
/// (unless `automountServiceAccountToken: false`). Its presence is what "in
/// Kubernetes" means here.
pub const NAMESPACE_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/namespace";

/// The namespace this process runs in, from [`NAMESPACE_FILE`]; `None` outside
/// Kubernetes.
pub fn pod_namespace() -> Option<String> {
    read_namespace(Path::new(NAMESPACE_FILE))
}

fn read_namespace(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The KMS alias that seals a new master key row for pods in `namespace`.
pub fn alias_for(namespace: &str) -> String {
    format!("alias/{namespace}-oxy-master-key")
}

/// What a KMS `DescribeKey` says about a key id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyLookup {
    Found,
    /// KMS answered `NotFoundException`.
    NotFound,
    /// This process has no AWS region or no AWS credentials, so it cannot ask.
    /// The text says which.
    NoAws(String),
}

/// The KMS calls this needs. A trait so the boot rule is testable without
/// AWS; [`AwsKms`] is the real one.
pub trait Kms {
    /// `DescribeKey` on `key_id`. `Err` for anything but the three answers
    /// [`KeyLookup`] names (access denied, KMS unreachable).
    fn find_key(&self, key_id: &str) -> impl Future<Output = Result<KeyLookup, String>> + Send;
    /// Seal `plaintext` under `key_id`: the ARN of the key that sealed it, and
    /// the ciphertext.
    fn encrypt(
        &self,
        key_id: &str,
        plaintext: &[u8],
    ) -> impl Future<Output = Result<(String, Vec<u8>), String>> + Send;
    /// Open `ciphertext` with no key id: KMS reads the key from it.
    /// `sealed_by`, the ARN stored with it, only picks the region to call.
    /// Returns the ARN of the key KMS opened it with, and the plaintext.
    fn decrypt(
        &self,
        sealed_by: &str,
        ciphertext: &[u8],
    ) -> impl Future<Output = Result<(String, Vec<u8>), String>> + Send;
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

/// What [`resolve`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// The master key from the row, and the ARN of the KMS key that sealed it.
    Stored { key: [u8; 32], sealed_by: String },
    /// The master key is not sealed by KMS: the variable, then the key file,
    /// as before. `warning` is logged once when there is one.
    NotSealed { warning: Option<String> },
}

fn kms_refusal(call: &str, key_id: &str, e: &str) -> String {
    format!(
        "KMS {call} on {key_id} failed, for the master key: {e}\n\n\
         This process's AWS credentials come from the IAM role bound to its Kubernetes service \
         account through EKS Pod Identity; that role needs kms:Encrypt, kms:Decrypt and \
         kms:DescribeKey on this key. \
         Refusing to start: without the master key every stored secret is unreadable, and no \
         other key will be used in its place."
    )
}

fn row_refusal(e: impl std::fmt::Display) -> String {
    format!(
        "{e}. Refusing to start rather than use another key; the row is written by the first \
         boot of a pod whose namespace has a KMS alias alias/<namespace>-oxy-master-key."
    )
}

fn as_key(bytes: Vec<u8>, sealed_by: &str) -> Result<[u8; 32], String> {
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
        row_refusal(format_args!(
            "server_keys row `{MASTER_KEY_ROW}` opened with KMS key {sealed_by} to {} bytes, \
             not the 32 of a master key",
            bytes.len()
        ))
    })
}

async fn open_row<K: Kms>(kms: &K, row: &[u8]) -> Result<([u8; 32], String), String> {
    let stored = StoredMasterKey::from_bytes(row).map_err(row_refusal)?;
    let (opened_by, plain) = kms
        .decrypt(&stored.sealed_by, &stored.ciphertext)
        .await
        .map_err(|e| kms_refusal("Decrypt", &stored.sealed_by, &e))?;
    if opened_by != stored.sealed_by {
        return Err(row_refusal(format_args!(
            "KMS opened server_keys row `{MASTER_KEY_ROW}` with key {opened_by}, but the row \
             says it was sealed by {}",
            stored.sealed_by
        )));
    }
    Ok((as_key(plain, &stored.sealed_by)?, stored.sealed_by))
}

/// The boot rule. `namespace` is the pod's ([`pod_namespace`]), `None` outside
/// Kubernetes. `may_seal` is false for `oxy migrate`, which only opens a row
/// already there. `variable` is `OXY_ENCRYPTION_KEY`, decoded, when set.
pub async fn resolve<K: Kms, S: MasterKeyStore>(
    kms: &K,
    store: &S,
    namespace: Option<&str>,
    may_seal: bool,
    variable: Option<[u8; 32]>,
) -> Result<Resolved, String> {
    let Some(namespace) = namespace else {
        return Ok(Resolved::NotSealed { warning: None });
    };
    if let Some(row) = store.read().await? {
        let (key, sealed_by) = open_row(kms, &row).await?;
        if let Some(variable) = variable
            && variable != key
        {
            return Err(format!(
                "{ENCRYPTION_KEY_VAR} is set and is not the master key stored in server_keys row \
                 `{MASTER_KEY_ROW}` (sealed by KMS key {sealed_by}). One of them is wrong. If the \
                 row was written by this deployment, remove {ENCRYPTION_KEY_VAR} or set it to \
                 the key the row holds; if the variable is the key this deployment's data was \
                 sealed with, the row is not this deployment's. Refusing to start rather than \
                 seal data under either."
            ));
        }
        tracing::info!(
            kms_key = %sealed_by,
            "master key opened from server_keys row `{MASTER_KEY_ROW}`"
        );
        return Ok(Resolved::Stored { key, sealed_by });
    }
    if !may_seal {
        return Ok(Resolved::NotSealed { warning: None });
    }
    let alias = alias_for(namespace);
    match kms
        .find_key(&alias)
        .await
        .map_err(|e| kms_refusal("DescribeKey", &alias, &e))?
    {
        KeyLookup::Found => {}
        KeyLookup::NotFound => {
            return Ok(Resolved::NotSealed {
                warning: Some(format!(
                    "the master key is not sealed by KMS because {alias} does not exist; \
                     using {ENCRYPTION_KEY_VAR} as before"
                )),
            });
        }
        KeyLookup::NoAws(why) => {
            return Ok(Resolved::NotSealed {
                warning: Some(format!(
                    "the master key is not sealed by KMS because this process cannot ask KMS \
                     whether {alias} exists ({why}); using {ENCRYPTION_KEY_VAR} as before"
                )),
            });
        }
    }
    let Some(variable) = variable else {
        return Err(format!(
            "{alias} exists and server_keys has no `{MASTER_KEY_ROW}` row yet, so this boot \
             needs {ENCRYPTION_KEY_VAR} to seal into it: set it to this deployment's current \
             key. Refusing to start rather than make a new key, which could not open the data \
             already here."
        ));
    };
    let (key, sealed_by) = seal_variable(kms, store, &alias, &variable).await?;
    Ok(Resolved::Stored { key, sealed_by })
}

/// No row yet: seal the variable into one, after it passes the data check,
/// and use whichever row ends up stored.
async fn seal_variable<K: Kms, S: MasterKeyStore>(
    kms: &K,
    store: &S,
    alias: &str,
    variable: &[u8; 32],
) -> Result<([u8; 32], String), String> {
    store.verify_variable(variable).await?;
    let (sealed_by, ciphertext) = kms
        .encrypt(alias, variable)
        .await
        .map_err(|e| kms_refusal("Encrypt", alias, &e))?;
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
    let (key, sealed_by) = open_row(kms, &row).await?;
    if key != *variable {
        return Err(format!(
            "Another process recorded a different master key in server_keys row \
             `{MASTER_KEY_ROW}` while this one was booting with {ENCRYPTION_KEY_VAR}. The \
             processes of this deployment do not hold the same {ENCRYPTION_KEY_VAR}; give every \
             one the same value."
        ));
    }
    tracing::info!(
        kms_key = alias,
        sealed_by = %sealed_by,
        "master key sealed by KMS and stored in server_keys"
    );
    Ok((key, sealed_by))
}

/// Set once this process has decided its master key is not sealed by KMS, so
/// the second boot call neither asks KMS again nor repeats the warning.
static NOT_SEALED: AtomicBool = AtomicBool::new(false);

/// Log `warning` unless `flag` was already set; sets it. `true` when it logged.
fn warn_once(flag: &AtomicBool, warning: Option<&str>) -> bool {
    if flag.swap(true, Ordering::SeqCst) {
        return false;
    }
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
        return true;
    }
    false
}

/// [`resolve`] with `kms`, then install the key for the life of the process.
/// Separate from [`load_at_boot`] so a test can pass a fake KMS.
pub async fn load_with<K: Kms, C: ConnectionTrait>(
    kms: &K,
    db: &C,
    namespace: Option<&str>,
    may_seal: bool,
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
    let (key, sealed_by) =
        match resolve(kms, &PostgresStore { db }, namespace, may_seal, variable).await? {
            Resolved::NotSealed { warning } => {
                warn_once(&NOT_SEALED, warning.as_deref());
                return Ok(());
            }
            Resolved::Stored { key, sealed_by } => (key, sealed_by),
        };
    install_master_key(key, MasterKeySource::Kms { key_id: sealed_by })?;
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

/// Load the master key from `server_keys` by the rule in this module's docs,
/// for `oxy serve` and `oxy worker`; nothing outside Kubernetes. Idempotent, and a no-op while
/// `server_keys` does not exist yet (a fresh database before migrations), so
/// entry points call it both before migrations (a migration may seal a value)
/// and after.
pub async fn load_at_boot<C: ConnectionTrait>(db: &C) -> Result<(), String> {
    load(db, true).await
}

/// [`load_at_boot`] that only opens a row already there and never seals a new
/// one, for `oxy migrate`: sealing stays with `oxy serve` and `oxy worker`,
/// which compare the row with their variable.
pub async fn open_at_boot<C: ConnectionTrait>(db: &C) -> Result<(), String> {
    load(db, false).await
}

async fn load<C: ConnectionTrait>(db: &C, may_seal: bool) -> Result<(), String> {
    if installed_master_key().is_some() || NOT_SEALED.load(Ordering::SeqCst) {
        return Ok(());
    }
    // Outside Kubernetes, as before: not even the read of the row.
    let Some(namespace) = pod_namespace() else {
        return Ok(());
    };
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
    load_with(&AwsKms::new(), db, Some(&namespace), may_seal).await
}
