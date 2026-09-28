//! Re-export shim. The connection-pool implementation lives in
//! `oxy-platform`; this module preserves the legacy `oxy::database::client`
//! import path so existing call sites compile unchanged.
pub use oxy_platform::db::{
    DatabaseAuthMode, DbFailure, IamConfig, establish_connection, listener_factory_from_env,
    listener_tls_verification_from_env,
};
