//! Re-export shim. The connection-pool implementation lives in
//! `oxy-platform`; this module preserves the legacy `oxy::database::client`
//! import path so existing call sites compile unchanged.
pub use oxy_platform::db::{
    DatabaseAuthMode, DbFailure, IamConfig, ListenerConnectFactory, SslMode, establish_connection,
    listener_factory_from_env, listener_ssl_mode_from_env,
};

/// `oxy-auth`'s platform port wired to this pool and the deployment's master
/// key: the one production [`oxy_auth::ports::AuthPlatform`], handed to every
/// auth entry point (`AuthState`, `BuiltInAuthenticator`, `session_key`, the
/// browser session). Each step still connects through `establish_connection`
/// at the point it always did.
pub fn auth_platform() -> oxy_auth::ports::AuthPlatform {
    oxy_auth::ports::AuthPlatform::new(
        establish_connection,
        oxy_platform::secrets::master_key_text_for_signing,
    )
}
