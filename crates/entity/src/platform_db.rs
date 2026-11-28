//! The kernel driven port for the platform database: [`PlatformDb`].
//!
//! Several crates reach the platform for the same things: a connection to
//! Oxy's own Postgres, the master-key text the session signing root mixes in,
//! and sealing a small value under the master key (`seal` / `open`, the
//! AES-256-GCM envelope Airhouse's service-account bearers and OLTP's
//! credentials are stored in). Calling `oxy_platform::db::establish_connection`,
//! `oxy_platform::secrets::master_key_text_for_signing` or
//! `oxy_platform::secrets::envelope` directly puts each of them above
//! `oxy-platform`. Taking this port instead lets them sit beside it
//! (`internal-docs/crate-tiers.md`, decision 5: a driven port shared by several
//! domains lives in the kernel).
//!
//! The one production implementation is `oxy_platform::ProcessPlatformDb`.
//!
//! The error type is the implementor's, not this crate's: `entity` names no
//! app error, and a connect error must reach the caller unchanged, so a port
//! user names it, as in `dyn PlatformDb<Error = OxyError>`.

use std::future::Future;
use std::pin::Pin;

use sea_orm::DatabaseConnection;

/// The future [`PlatformDb::connect`] returns.
pub type ConnectFuture<'a, E> =
    Pin<Box<dyn Future<Output = Result<DatabaseConnection, E>> + Send + 'a>>;

/// The platform database and the master key, as one driven port.
///
/// The envelope is here, and not moved down beside this trait, because its key
/// is process state only `oxy-platform` holds: the master key it installs at
/// boot from KMS, or the environment variable, or the dev key file.
pub trait PlatformDb: Send + Sync {
    /// What a failed connect reports.
    type Error;

    /// Opens (or reuses) the connection to the platform database.
    fn connect(&self) -> ConnectFuture<'_, Self::Error>;

    /// The master-key text the session signing root mixes in, read at the
    /// moment it is asked for. `None` when the process has no master key.
    fn master_key_text_for_signing(&self) -> Option<String>;

    /// Seals `plaintext` under the master key, read at the moment it is asked
    /// for. Returns the self-describing blob `open` takes back.
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, Self::Error>;

    /// Opens a blob `seal` produced, under the master key read at the moment
    /// it is asked for.
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoDatabase;

    impl PlatformDb for NoDatabase {
        type Error = String;

        fn connect(&self) -> ConnectFuture<'_, String> {
            Box::pin(async { Err("no database".to_string()) })
        }

        fn master_key_text_for_signing(&self) -> Option<String> {
            Some("key".to_string())
        }

        fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
            Ok(plaintext.to_vec())
        }

        fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
            Ok(sealed.to_vec())
        }
    }

    /// The port is used as `Arc<dyn PlatformDb<Error = ..>>`, so it must stay
    /// dyn-compatible and shareable across threads.
    #[test]
    fn the_port_is_dyn_compatible_and_shareable() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn PlatformDb<Error = String>>();

        let port: std::sync::Arc<dyn PlatformDb<Error = String>> = std::sync::Arc::new(NoDatabase);
        assert_eq!(port.master_key_text_for_signing().as_deref(), Some("key"));
    }
}
