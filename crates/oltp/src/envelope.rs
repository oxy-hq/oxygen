//! Sealing a credential under the master key, through the kernel port the
//! binary installed ([`entity::platform_db::install_platform_db`]), not
//! `oxy-platform` itself. `oxy-server` installs `oxy_platform::ProcessPlatformDb`
//! at boot; a test binary or example that seals or opens one installs it too.
//!
//! The error is the platform's, unchanged, so each caller's message reads as it
//! did when this called `oxy_platform::secrets::envelope`.

use ::entity::platform_db::{InstalledPlatformDb, PlatformDb};
use oxy_shared::errors::OxyError;

pub(crate) fn seal(plaintext: &[u8]) -> Result<Vec<u8>, OxyError> {
    PlatformDb::seal(&InstalledPlatformDb::<OxyError>::default(), plaintext)
}

pub(crate) fn open(sealed: &[u8]) -> Result<Vec<u8>, OxyError> {
    PlatformDb::open(&InstalledPlatformDb::<OxyError>::default(), sealed)
}
