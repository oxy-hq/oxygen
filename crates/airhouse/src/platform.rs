//! The platform this crate reaches through the kernel port
//! [`entity::platform_db::PlatformDb`]: Oxy's own Postgres (where
//! `airhouse_tenants` lives) and the master key the tenants' service-account
//! bearers are sealed under.
//!
//! The broker and the provisioner take the port. Their process-wide factories
//! ([`crate::token_broker`], [`crate::provisioner_for`]) hand them
//! [`InstalledPlatformDb`], which forwards each call to the port the binary
//! installed with [`install_platform_db`]. `oxy-server` installs
//! `oxy_platform::ProcessPlatformDb` in `cli::prepare`, before any command
//! runs; a test binary that enables Airhouse and goes through the factories
//! installs it the same way.
//!
//! The forwarding is late-bound on purpose: the broker is built once, at its
//! first use, and an install that comes after that must still reach it.

use std::sync::{Arc, OnceLock};

use entity::platform_db::{ConnectFuture, PlatformDb};
use oxy_shared::errors::OxyError;

/// The port as the broker and the provisioner hold it.
pub type SharedPlatformDb = Arc<dyn PlatformDb<Error = OxyError>>;

static INSTALLED: OnceLock<SharedPlatformDb> = OnceLock::new();

const NOT_INSTALLED: &str = "airhouse: no platform database port is installed. oxy-server \
     installs oxy_platform::ProcessPlatformDb at boot (airhouse::install_platform_db); \
     a test binary that enables Airhouse must install it too";

/// Installs the port the process-wide broker and provisioner use. The first
/// install wins and later ones are ignored, so calling it twice is harmless.
/// Returns whether this call installed it.
pub fn install_platform_db(platform: SharedPlatformDb) -> bool {
    INSTALLED.set(platform).is_ok()
}

/// The port [`install_platform_db`] installed, looked up at each call. Without
/// an install, every call fails with an error that says so, rather than reading
/// as "Airhouse is not configured".
#[derive(Debug, Clone, Copy, Default)]
pub struct InstalledPlatformDb;

impl InstalledPlatformDb {
    /// A shared handle, as the broker and the provisioner take it.
    pub fn shared() -> SharedPlatformDb {
        Arc::new(Self)
    }
}

fn installed() -> Result<&'static SharedPlatformDb, OxyError> {
    INSTALLED
        .get()
        .ok_or_else(|| OxyError::RuntimeError(NOT_INSTALLED.to_string()))
}

impl PlatformDb for InstalledPlatformDb {
    type Error = OxyError;

    fn connect(&self) -> ConnectFuture<'_, OxyError> {
        match installed() {
            Ok(platform) => PlatformDb::connect(&**platform),
            Err(e) => Box::pin(async move { Err(e) }),
        }
    }

    fn master_key_text_for_signing(&self) -> Option<String> {
        INSTALLED
            .get()
            .and_then(|platform| PlatformDb::master_key_text_for_signing(&**platform))
    }

    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, OxyError> {
        PlatformDb::seal(&**installed()?, plaintext)
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, OxyError> {
        PlatformDb::open(&**installed()?, sealed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing in this crate's unit tests installs a port, so the late-bound
    /// one reports that, distinctly, on every operation.
    #[tokio::test]
    async fn without_an_install_every_operation_says_so() {
        let port = InstalledPlatformDb::shared();
        let connect = PlatformDb::connect(&*port).await.expect_err("no port");
        assert!(
            connect.to_string().contains("no platform database port"),
            "{connect}"
        );
        let seal = PlatformDb::seal(&*port, b"x").expect_err("no port");
        assert!(
            seal.to_string().contains("no platform database port"),
            "{seal}"
        );
        let open = PlatformDb::open(&*port, b"x").expect_err("no port");
        assert!(
            open.to_string().contains("no platform database port"),
            "{open}"
        );
        assert_eq!(PlatformDb::master_key_text_for_signing(&*port), None);
    }
}
