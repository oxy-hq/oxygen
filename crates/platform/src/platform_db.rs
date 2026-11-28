//! The production [`PlatformDb`]: this process's platform database pool and
//! its master key.

use entity::platform_db::{ConnectFuture, PlatformDb};
use oxy_shared::errors::OxyError;

/// The one production implementation of the kernel port
/// [`entity::platform_db::PlatformDb`]. It holds nothing: each call goes to
/// the process-wide state the free functions read, so a connect shares the
/// one pool [`crate::db::establish_connection`] initializes, and the master
/// key is read when asked for (after boot has installed a KMS key), exactly as
/// [`crate::secrets::master_key_text_for_signing`] reads it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessPlatformDb;

impl PlatformDb for ProcessPlatformDb {
    type Error = OxyError;

    fn connect(&self) -> ConnectFuture<'_, OxyError> {
        Box::pin(crate::db::establish_connection())
    }

    fn master_key_text_for_signing(&self) -> Option<String> {
        crate::secrets::master_key_text_for_signing()
    }
}
