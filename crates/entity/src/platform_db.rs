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
//! Its one install, [`install_platform_db`], read back through the late-bound
//! [`InstalledPlatformDb`], is shared by Airhouse and OLTP.

use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

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

/// What every operation of [`InstalledPlatformDb`] fails with when nothing is
/// installed, so it never reads as "not configured".
pub const NOT_INSTALLED: &str = "no platform database port is installed. oxy-server \
     installs oxy_platform::ProcessPlatformDb at boot (ProcessPlatformDb::install); a \
     test binary that reaches Airhouse or OLTP through the installed port must install it too";

// A static cannot be generic, so the install is type-erased: it holds the
// `Arc<dyn PlatformDb<Error = E>>` it was given, and `InstalledPlatformDb<E>`
// finds it by that type. A process installs one port; a lookup under another
// error type reads as not installed.
static INSTALLED: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();

/// Installs the port [`InstalledPlatformDb`] forwards to. The first install
/// wins and later ones are ignored, so calling it twice is harmless. Returns
/// whether this call installed it.
pub fn install_platform_db<E: 'static>(platform: Arc<dyn PlatformDb<Error = E>>) -> bool {
    INSTALLED.set(Box::new(platform)).is_ok()
}

fn installed<E: From<String> + 'static>() -> Result<&'static Arc<dyn PlatformDb<Error = E>>, E> {
    let found = INSTALLED.get().and_then(|any| any.downcast_ref());
    found.ok_or_else(|| E::from(NOT_INSTALLED.to_string()))
}

/// The port [`install_platform_db`] installed, looked up at each call: a
/// handle built before the install still reaches it.
pub struct InstalledPlatformDb<E>(PhantomData<fn() -> E>);

impl<E> Default for InstalledPlatformDb<E> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<E: From<String> + Send + 'static> InstalledPlatformDb<E> {
    /// A shared handle, as a port user holds one.
    pub fn shared() -> Arc<dyn PlatformDb<Error = E>> {
        Arc::new(Self::default())
    }
}

impl<E: From<String> + Send + 'static> PlatformDb for InstalledPlatformDb<E> {
    type Error = E;

    fn connect(&self) -> ConnectFuture<'_, E> {
        match installed::<E>() {
            Ok(platform) => PlatformDb::connect(&**platform),
            Err(e) => Box::pin(std::future::ready(Err(e))),
        }
    }

    fn master_key_text_for_signing(&self) -> Option<String> {
        let platform = installed::<E>().ok()?;
        PlatformDb::master_key_text_for_signing(&**platform)
    }

    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, E> {
        PlatformDb::seal(&**installed::<E>()?, plaintext)
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, E> {
        PlatformDb::open(&**installed::<E>()?, sealed)
    }
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

    /// Nothing in this crate's tests installs a port, so the late-bound one
    /// reports that, distinctly, on every operation.
    #[test]
    fn without_an_install_every_operation_says_so() {
        let port = InstalledPlatformDb::<String>::shared();
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(Err(connect)) =
            PlatformDb::connect(&*port).as_mut().poll(&mut cx)
        else {
            panic!("a missing port must fail the connect at once");
        };
        assert!(connect.contains("no platform database port"), "{connect}");
        let seal = PlatformDb::seal(&*port, b"x").expect_err("no port");
        assert!(seal.contains("no platform database port"), "{seal}");
        let open = PlatformDb::open(&*port, b"x").expect_err("no port");
        assert!(open.contains("no platform database port"), "{open}");
        assert_eq!(PlatformDb::master_key_text_for_signing(&*port), None);
    }
}
