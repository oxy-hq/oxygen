//! The keys that sign browser sessions — one set per deployment, none of it in
//! the source.
//!
//! Sessions used to be signed with a string constant. It was the same on every
//! deployment and in the public repository, so anyone could sign a session for
//! any user, anywhere, and a session minted on dev was valid on prod. This
//! module is what replaced it.
//!
//! ## Where the key comes from
//!
//! 1. **A random secret in Postgres** (`server_keys`, row `session`), created
//!    by whichever instance asks first. Every instance of a deployment shares
//!    one database, so they agree on it by construction — which no environment
//!    variable or key file can promise: a pod that is missing the variable, or
//!    fabricates its own file, signs sessions its neighbours refuse, and that
//!    reads as "signed out at random".
//! 2. **Mixed with `OXY_ENCRYPTION_KEY`, when that variable is set.** Then a
//!    copy of the database alone — a backup, a read replica, a leaked dump — is
//!    not enough to sign a session. Only the variable is used, never the
//!    state-dir key file [`oxy_platform::secrets`] falls back to: the variable
//!    is the same on every pod of a fleet, the file is not.
//!
//! Each [`Purpose`] is signed with a key of its own, derived from that root, so
//! an OAuth `state` can never be replayed as a session.
//!
//! ## What changing any of it does
//!
//! Every session ends, once: people sign in again. API tokens are not sessions
//! and are untouched, and a token session (`token::browser_session`) is signed
//! by its own token. That is the cost of deploying this, of rotating the row
//! (`DELETE FROM server_keys WHERE name = 'session'`, then restart), and of
//! adding, removing or rotating `OXY_ENCRYPTION_KEY`.
//!
//! Because of (2), an instance that is missing the variable while its
//! neighbours have it derives a different root, and its sessions are refused
//! next door. Each instance logs a `key_fingerprint` when it loads the root;
//! across one deployment they must all be the same.
//!
//! The root is read once per process. A failed read is not cached, so a
//! database that was briefly away costs one refused request, not a restart.

use entity::prelude::ServerKeys;
use jsonwebtoken::{DecodingKey, EncodingKey};
use oxy_platform::db::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

/// `server_keys.name` of the row every session key is derived from.
const SESSION_ROW: &str = "session";
/// The fleet-wide master key, when the deployment sets one. Read as text and
/// mixed in as-is; whether it decodes is `oxy_platform::secrets`' business.
const MASTER_KEY_VAR: &str = "OXY_ENCRYPTION_KEY";
const ROOT_DOMAIN: &[u8] = b"oxy-signing-root:v1\0";

/// What a key signs. One key each: a JWT signed for one purpose does not
/// verify for another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A browser session: the `Authorization` JWT and the `oxy_session` cookie.
    Session,
    /// The CSRF `state` an OAuth sign-in carries to the provider and back.
    OAuthState,
}

impl Purpose {
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Session => b"oxy-session-jwt:v1\0",
            Self::OAuthState => b"oxy-oauth-state:v1\0",
        }
    }
}

static ROOT: OnceCell<[u8; 32]> = OnceCell::const_new();

/// The root, from the database's secret and the master key if there is one.
fn root_from(stored: &[u8], master: Option<&str>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ROOT_DOMAIN);
    hasher.update((stored.len() as u64).to_be_bytes());
    hasher.update(stored);
    hasher.update(master.unwrap_or_default().as_bytes());
    hasher.finalize().into()
}

/// Eight hex digits that name a root without revealing it: a hash of the
/// root under its own domain, cut to four bytes. For comparing instances in
/// the logs, nothing else.
fn fingerprint(root: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"oxy-signing-fingerprint:v1\0");
    hasher.update(root);
    hasher.finalize()[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn derive(root: &[u8; 32], purpose: Purpose) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(purpose.domain());
    hasher.update(root);
    hasher.finalize().into()
}

fn unavailable(what: &str, e: impl std::fmt::Display) -> OxyError {
    tracing::error!("session signing key unavailable ({what}): {e}");
    OxyError::AuthenticationError("Session signing key unavailable".to_string())
}

/// The deployment's stored secret, creating it if this is the first instance
/// to ask. Insert-if-absent then read, so two instances starting together end
/// up holding the same row.
async fn stored_secret<C: ConnectionTrait>(db: &C) -> Result<Vec<u8>, OxyError> {
    let fresh: [u8; 32] = rand::random();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO server_keys (name, secret) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING",
        [SESSION_ROW.into(), fresh.to_vec().into()],
    ))
    .await
    .map_err(|e| unavailable("store", e))?;
    ServerKeys::find_by_id(SESSION_ROW)
        .one(db)
        .await
        .map_err(|e| unavailable("read", e))?
        .map(|row| row.secret)
        .filter(|secret| secret.len() >= 32)
        .ok_or_else(|| unavailable("read", "no usable row"))
}

async fn load_root() -> Result<[u8; 32], OxyError> {
    let db = establish_connection()
        .await
        .map_err(|e| unavailable("connect", e))?;
    let stored = stored_secret(&db).await?;
    let master = std::env::var(MASTER_KEY_VAR).ok().filter(|v| !v.is_empty());
    let root = root_from(&stored, master.as_deref());
    // Once per process, and never the key. Every instance of a deployment must
    // print the same fingerprint: one that differs is an instance whose
    // `OXY_ENCRYPTION_KEY` is not its neighbours', and its sessions are the
    // ones being refused.
    tracing::info!(
        mixed_with_master_key = master.is_some(),
        key_fingerprint = %fingerprint(&root),
        "session signing key loaded from server_keys"
    );
    Ok(root)
}

/// The raw key for `purpose`. Reads the database on the first call of the
/// process and never again.
pub async fn key(purpose: Purpose) -> Result<[u8; 32], OxyError> {
    let root = ROOT.get_or_try_init(load_root).await?;
    Ok(derive(root, purpose))
}

/// The key to sign a `purpose` JWT with.
pub async fn encoding_key(purpose: Purpose) -> Result<EncodingKey, OxyError> {
    Ok(EncodingKey::from_secret(&key(purpose).await?))
}

/// The key to verify a `purpose` JWT with.
pub async fn decoding_key(purpose: Purpose) -> Result<DecodingKey, OxyError> {
    Ok(DecodingKey::from_secret(&key(purpose).await?))
}

/// Fix the root for this process without a database — for a test that signs or
/// verifies a session and has none. First call wins; later ones are ignored.
/// Nothing in the server calls this.
#[doc(hidden)]
pub fn install_root_for_tests(root: [u8; 32]) {
    let _ = ROOT.set(root);
}

/// A fixed key for this crate's own tests: installs a root and derives from
/// it, so a test can sign a session and have [`key`] verify it, synchronously
/// and with no database.
#[cfg(test)]
pub(crate) fn test_key(purpose: Purpose) -> [u8; 32] {
    install_root_for_tests([42; 32]);
    derive(ROOT.get().expect("a root was just installed"), purpose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_purpose_has_a_key_of_its_own() {
        let root = root_from(&[7; 32], None);
        assert_ne!(
            derive(&root, Purpose::Session),
            derive(&root, Purpose::OAuthState)
        );
        // And neither is the root: a leaked purpose key gives up no other.
        assert_ne!(derive(&root, Purpose::Session), root);
    }

    #[test]
    fn a_fingerprint_tells_two_roots_apart_and_is_not_part_of_either() {
        let (a, b) = (root_from(&[1; 32], None), root_from(&[1; 32], Some("k")));
        assert_eq!(fingerprint(&a).len(), 8);
        assert_eq!(fingerprint(&a), fingerprint(&a));
        // The case it exists for: the same database, one pod missing the key.
        assert_ne!(fingerprint(&a), fingerprint(&b));
        let hex_of_root: String = a.iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(!hex_of_root.contains(&fingerprint(&a)));
    }

    #[test]
    fn two_deployments_do_not_share_a_key() {
        // Different databases hold different secrets: a session minted on one
        // deployment is not valid on another, which the old constant allowed.
        assert_ne!(root_from(&[1; 32], None), root_from(&[2; 32], None));
    }

    #[test]
    fn the_master_key_is_part_of_the_root_when_there_is_one() {
        let stored = [9; 32];
        let alone = root_from(&stored, None);
        let with_master = root_from(&stored, Some("bWFzdGVyLWtleQ=="));
        // The database's secret alone no longer signs anything…
        assert_ne!(alone, with_master);
        // …and it is the same master key, not just any, that makes the root.
        assert_ne!(with_master, root_from(&stored, Some("another")));
        assert_eq!(with_master, root_from(&stored, Some("bWFzdGVyLWtleQ==")));
    }
}
