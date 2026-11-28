//! Re-export shim. The connection-pool implementation lives in
//! `oxy-platform`; this module preserves the legacy `oxy::database::client`
//! import path so existing call sites compile unchanged.
pub use oxy_platform::db::{
    DatabaseAuthMode, DbFailure, IamConfig, ListenerConnectFactory, SslMode, establish_connection,
    listener_factory_from_env, listener_ssl_mode_from_env,
};
