//! AES-256 master key loader.
//!
//! Looks up the key in this order:
//! 1. `OXY_ENCRYPTION_KEY` env var (base64-encoded 32 bytes).
//! 2. `<state-dir>/encryption_key.txt` (created on first call when the env
//!    var is unset — convenient for local dev, **not safe for production**).
//! 3. If neither exists, generates a fresh key and writes it to the file.
//!
//! Steps 2 and 3 are for a single process (`OXY_ROLE=all`, `oxy start`).
//! `oxy serve` and `oxy worker` refuse to start in any other role without the
//! env var — see `oxy_app::server::shared_state_check` — because a file key is
//! per node, and a second node would mint its own and fail to read what the
//! first sealed.
//!
//! Step 3 is also refused, as a panic naming the variable, in any process that
//! is not the single process: one whose `OXY_ROLE` is `ide`, `serve` or
//! `worker` (a CLI subcommand run inside a fleet pod inherits it), or one that
//! called [`forbid_key_file_creation`] (`oxy serve` and `oxy worker` in a fleet
//! role, and `oxy migrate`, which runs as the chart's migrate Job). A key made
//! there seals values that nothing else can open.
//!
//! ## A set key that is the wrong key
//!
//! Every boot of `oxy serve` and `oxy worker`, in every role, opens a value
//! sealed with this key and stored in Postgres (`server_keys` row
//! `encryption_check`) and refuses to start when it does not open — see
//! [`super::key_check`]. That is what catches a variable that is set but is
//! not the key this deployment's data was sealed with (an edited Secret, a
//! value copied from another environment, a restore): without it every sealed
//! value becomes unreadable while the pods report healthy.
//!
//! ## The key in Postgres, sealed by KMS
//!
//! In Kubernetes, the master key can be a `server_keys` row (`master_key`)
//! sealed by KMS, loaded once at boot by [`super::master_key`] and installed
//! here ([`install_master_key`]); from then on [`get_encryption_key`] returns
//! it and nothing else. Nothing is configured for it: a row that is there is
//! opened, and with no row the first boot of a pod whose namespace has a KMS
//! alias `alias/<namespace>-oxy-master-key` seals `OXY_ENCRYPTION_KEY` into
//! it, after [`super::key_check`] has shown it opens the existing data; after
//! that the variable may be removed. Without the alias, or outside
//! Kubernetes, the steps above apply as before.
//!
//! Rotation (opening with a previous key and sealing again with a new one) is
//! not implemented; it would go in [`super::key_check`], which is the one place
//! that knows which key a deployment's data was sealed with.
//!
//! The master key is consumed by [`crate::secrets::OrgSecretsService`] for
//! per-org secret encryption and by other oxy modules that encrypt their
//! own values (Airhouse and OLTP credentials, camera device secrets, the
//! legacy `_var` secret manager).

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use aes_gcm::aead::Generate;
use aes_gcm::{Aes256Gcm, Key};
use base64::{Engine as _, engine::general_purpose, engine::general_purpose::STANDARD as BASE64};
use oxy_shared::errors::OxyError;

/// The env var holding the master key: base64 of 32 bytes.
pub const ENCRYPTION_KEY_VAR: &str = "OXY_ENCRYPTION_KEY";

/// Where a key installed with [`install_master_key`] came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MasterKeySource {
    /// `server_keys` row `master_key`, sealed by this KMS key (an ARN).
    Kms { key_id: String },
}

struct Installed {
    key: [u8; 32],
    source: MasterKeySource,
}

/// The master key this process loaded at boot, when it loads it from somewhere
/// other than the variable or the file. Once set, it is what
/// [`get_encryption_key`] returns for the life of the process.
static INSTALLED: OnceLock<Installed> = OnceLock::new();

/// Hold `key` as this process's master key from now on. A second call with
/// the same key is a no-op; with a different key it is an error, since two
/// keys in one process is exactly what this module exists to prevent.
pub fn install_master_key(key: [u8; 32], source: MasterKeySource) -> Result<(), String> {
    let installed = INSTALLED.get_or_init(|| Installed { key, source });
    if installed.key == key {
        Ok(())
    } else {
        Err("a different master key is already loaded in this process".to_string())
    }
}

/// The key [`install_master_key`] installed, if any.
pub fn installed_master_key() -> Option<[u8; 32]> {
    INSTALLED.get().map(|i| i.key)
}

/// The text `oxy_auth::session_key` mixes into the session-signing root.
///
/// While `OXY_ENCRYPTION_KEY` is set, its raw value, exactly as that module
/// has always read it. Without the variable, the base64 of a master key loaded
/// from KMS, which is the same text whenever the variable held the canonical
/// encoding of the same key (the usual `openssl rand -base64 32` value). So
/// removing the variable once the key is in `server_keys` keeps every session.
/// A key from the key file is never mixed in, as before.
pub fn master_key_text_for_signing() -> Option<String> {
    signing_text(
        std::env::var(ENCRYPTION_KEY_VAR).ok(),
        INSTALLED.get().map(|i| i.key),
    )
}

fn signing_text(variable: Option<String>, installed: Option<[u8; 32]>) -> Option<String> {
    variable
        .filter(|v| !v.is_empty())
        .or_else(|| installed.map(|key| BASE64.encode(key)))
}

/// Look up oxy's state directory.
///
/// Mirrors `oxy::state_dir::get_state_dir` — duplicated here (rather than
/// imported) because that function lives in the `oxy` crate, which depends
/// on `oxy-platform`. Pulling it in would invert the dependency direction.
fn get_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OXY_STATE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .map(|d| d.join("oxy"))
        .unwrap_or_else(|| PathBuf::from(".oxy_state"))
}

fn get_key_file_path() -> PathBuf {
    get_state_dir().join("encryption_key.txt")
}

/// Set once by an entry point that is not the single process; names it in the
/// refusal.
static KEY_FILE_CREATION_FORBIDDEN_BY: OnceLock<String> = OnceLock::new();

/// Never create `encryption_key.txt` in this process. `by` names the process
/// for the panic message, e.g. `"OXY_ROLE=worker"` or `"oxy migrate"`. First
/// call wins.
pub fn forbid_key_file_creation(by: impl Into<String>) {
    let _ = KEY_FILE_CREATION_FORBIDDEN_BY.set(by.into());
}

/// Who forbids creating the key file in this process, if anyone: an entry point
/// that called [`forbid_key_file_creation`], or an `OXY_ROLE` naming a fleet
/// role. `role` is the value of `OXY_ROLE`, passed in so this is testable.
fn key_file_creation_forbidden_by(role: Option<&str>) -> Option<String> {
    if let Some(by) = KEY_FILE_CREATION_FORBIDDEN_BY.get() {
        return Some(by.clone());
    }
    match role.map(str::trim) {
        Some(role @ ("ide" | "serve" | "worker")) => Some(format!("OXY_ROLE={role}")),
        _ => None,
    }
}

/// Where [`get_encryption_key`] takes the key from, for a message: the key
/// loaded from KMS, the variable when it is set, otherwise the key file.
pub fn encryption_key_source() -> String {
    if let Some(installed) = INSTALLED.get() {
        let MasterKeySource::Kms { key_id } = &installed.source;
        return format!("server_keys row `master_key` (sealed by KMS key {key_id})");
    }
    if std::env::var(ENCRYPTION_KEY_VAR).is_ok() {
        ENCRYPTION_KEY_VAR.to_string()
    } else {
        format!("the key file {}", get_key_file_path().display())
    }
}

/// Decode a master key without panicking: base64 of exactly 32 bytes.
///
/// The one definition of "a valid key". [`get_encryption_key`] panics on a
/// value this refuses; a boot check calls this first so an operator gets the
/// reason as a startup error instead of a panic on the first secret read.
pub fn parse_encryption_key(key_str: &str) -> Result<[u8; 32], String> {
    let decoded = general_purpose::STANDARD
        .decode(key_str.trim())
        .map_err(|e| format!("not valid base64: {e}"))?;
    if decoded.len() != 32 {
        return Err(format!(
            "expected 32 bytes after base64 decoding, got {}",
            decoded.len()
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded);
    Ok(key)
}

fn decode_key_from_string(key_str: &str) -> [u8; 32] {
    parse_encryption_key(key_str)
        .map_err(|e| OxyError::SecretManager(format!("Invalid encryption key format: {e}")))
        .expect("Failed to decode encryption key")
}

/// Get the encryption key from environment variable.
/// Falls back to a development key for development (NOT secure for production).
pub fn get_encryption_key() -> [u8; 32] {
    // The key loaded at boot, when there is one: the process holds it from
    // then on, and boot already refused a variable that differs from it.
    if let Some(installed) = INSTALLED.get() {
        return installed.key;
    }
    // Then the environment variable
    if let Ok(key_str) = std::env::var(ENCRYPTION_KEY_VAR) {
        return decode_key_from_string(&key_str);
    }
    let key_file_path = get_key_file_path();
    if let Ok(key_str) = fs::read_to_string(&key_file_path) {
        let key_str = key_str.trim();
        if !key_str.is_empty() {
            tracing::info!("Loading encryption key from file: {:?}", key_file_path);
            return decode_key_from_string(key_str);
        }
    }

    if let Some(by) = key_file_creation_forbidden_by(std::env::var("OXY_ROLE").ok().as_deref()) {
        panic!(
            "{ENCRYPTION_KEY_VAR} is not set, and {by} may not create a key file of its own \
             ({}): a key made here would seal values that no other process can open. \
             Set {ENCRYPTION_KEY_VAR} to this deployment's key.",
            key_file_path.display()
        );
    }

    let key = Key::<Aes256Gcm>::generate();

    // Ensure directory exists
    if let Some(parent) = key_file_path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        tracing::error!("Failed to create directory for encryption key: {}", e);
    }
    let key_string = BASE64.encode(key);

    if let Err(e) = fs::write(&key_file_path, &key_string) {
        tracing::error!("Failed to save encryption key to file: {}", e);
    } else {
        tracing::info!(
            "Generated new encryption key and saved to: {:?}",
            key_file_path
        );
    }

    tracing::warn!(
        "No encryption key found. Generated new key and saved to: {:?}",
        key_file_path
    );
    key.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `forbid_key_file_creation` is process-wide and never called in this
    // test binary, so only the `OXY_ROLE` half is exercised here; the boot
    // test in `oxy-app` (`shared_state_boot`) covers the entry points.
    #[test]
    fn a_fleet_role_may_not_create_the_key_file() {
        for role in ["ide", "serve", "worker", " worker "] {
            assert_eq!(
                key_file_creation_forbidden_by(Some(role)).as_deref(),
                Some(format!("OXY_ROLE={}", role.trim()).as_str()),
            );
        }
    }

    /// Removing the variable once the key is loaded from KMS must not change
    /// what the session-signing root is derived from.
    #[test]
    fn the_signing_text_is_the_same_with_the_variable_or_the_loaded_key() {
        let key = [9u8; 32];
        let variable = BASE64.encode(key); // what `openssl rand -base64 32` prints
        let before = signing_text(Some(variable.clone()), None);
        assert_eq!(signing_text(Some(variable.clone()), Some(key)), before);
        assert_eq!(signing_text(None, Some(key)), before);
        assert_eq!(signing_text(Some(String::new()), None), None);
        assert_eq!(signing_text(None, None), None);
    }

    #[test]
    fn the_single_process_may() {
        for role in [None, Some("all"), Some(""), Some("something-else")] {
            assert_eq!(key_file_creation_forbidden_by(role), None, "{role:?}");
        }
    }
}
