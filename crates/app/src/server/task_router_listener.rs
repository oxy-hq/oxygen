//! Wires the platform's database settings into the agentic task router's
//! LISTEN connection.
//!
//! `oxy-platform` (re-exported as `oxy::database::client`) owns the credentials (password URL or IAM token mint) and
//! the `OXY_DATABASE_SSL_MODE` contract, but may not name an `agentic-runtime`
//! type (`internal-docs/domain-boundaries.md` L2). This crate sits above both,
//! so the mapping between them lives here.

use agentic_runtime::router::{ListenerConfigFactory, TlsVerification};
use oxy::database::client::{SslMode, listener_ssl_mode_from_env};
use oxy_shared::errors::OxyError;

/// The router's connection factory, honouring `OXY_DATABASE_AUTH_MODE`
/// exactly as the pool does.
pub(crate) fn listener_factory_from_env() -> Result<ListenerConfigFactory, OxyError> {
    oxy::database::client::listener_factory_from_env()
}

/// The listener's certificate-verification posture, matching the pool's
/// `OXY_DATABASE_SSL_MODE`: `require` encrypts without validating (our RDS
/// and CloudNativePG CAs aren't in the Mozilla bundle); `verify-full`
/// enforces the full chain and hostname.
pub(crate) fn listener_tls_verification_from_env() -> Result<TlsVerification, OxyError> {
    Ok(tls_verification(listener_ssl_mode_from_env()?))
}

fn tls_verification(mode: SslMode) -> TlsVerification {
    match mode {
        SslMode::Require => TlsVerification::RequireNoVerify,
        SslMode::VerifyFull => TlsVerification::VerifyFull,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_encrypts_without_verifying_and_verify_full_verifies() {
        assert_eq!(
            tls_verification(SslMode::Require),
            TlsVerification::RequireNoVerify
        );
        assert_eq!(
            tls_verification(SslMode::VerifyFull),
            TlsVerification::VerifyFull
        );
    }
}
