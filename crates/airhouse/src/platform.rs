//! The platform this crate reaches through the kernel port
//! [`entity::platform_db::PlatformDb`]: Oxy's own Postgres (where
//! `airhouse_tenants` lives) and the master key the tenants' service-account
//! bearers are sealed under.
//!
//! The broker and the provisioner take the port. Their process-wide factories
//! ([`crate::token_broker`], [`crate::provisioner_for`]) hand them
//! [`InstalledPlatformDb`], which forwards each call to the port the binary
//! installed in the kernel ([`install_platform_db`], shared with OLTP).
//! `oxy-server` installs `oxy_platform::ProcessPlatformDb` in `cli::prepare`,
//! before any command runs; a test binary that enables Airhouse and goes
//! through the factories installs it the same way.
//!
//! The forwarding is late-bound on purpose: the broker is built once, at its
//! first use, and an install that comes after that must still reach it.

use std::sync::Arc;

use entity::platform_db::PlatformDb;
use oxy_shared::errors::OxyError;

pub use entity::platform_db::install_platform_db;

/// The port as the broker and the provisioner hold it.
pub type SharedPlatformDb = Arc<dyn PlatformDb<Error = OxyError>>;

/// The kernel's late-bound handle, with this crate's error.
pub type InstalledPlatformDb = entity::platform_db::InstalledPlatformDb<OxyError>;
