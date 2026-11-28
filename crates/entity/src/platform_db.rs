//! The kernel driven port for the platform database: [`PlatformDb`].
//!
//! Several crates reach the platform for the same two things: a connection to
//! Oxy's own Postgres, and the master-key text the session signing root mixes
//! in. Calling `oxy_platform::db::establish_connection` and
//! `oxy_platform::secrets::master_key_text_for_signing` directly puts each of
//! them above `oxy-platform`. Taking this port instead lets them sit beside it
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
pub trait PlatformDb: Send + Sync {
    /// What a failed connect reports.
    type Error;

    /// Opens (or reuses) the connection to the platform database.
    fn connect(&self) -> ConnectFuture<'_, Self::Error>;

    /// The master-key text the session signing root mixes in, read at the
    /// moment it is asked for. `None` when the process has no master key.
    fn master_key_text_for_signing(&self) -> Option<String>;
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
