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
//! The master key is consumed by [`crate::secrets::OrgSecretsService`] for
//! per-org secret encryption and by other oxy modules that encrypt their
//! own values (GitHub OAuth tokens, the legacy `_var` secret manager).

use std::fs;
use std::path::PathBuf;

use aes_gcm::aead::Generate;
use aes_gcm::{Aes256Gcm, Key};
use base64::{Engine as _, engine::general_purpose, engine::general_purpose::STANDARD as BASE64};
use oxy_shared::errors::OxyError;

/// The env var holding the master key: base64 of 32 bytes.
pub const ENCRYPTION_KEY_VAR: &str = "OXY_ENCRYPTION_KEY";

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
    // First try environment variable
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
