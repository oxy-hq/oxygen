//! GitHub Actions OIDC: the one verifier behind both exchanges.
//!
//! A CI job presents a JWT GitHub signed for it. Two routes trade one for an
//! Oxy credential, and both come through here:
//!
//! - the legacy trusted-publishing exchange
//!   (`POST /api/customer-apps/publish/oidc-exchange`, audience
//!   [`AUDIENCE_PUBLISH`]), which mints an app-scoped `oxypublish_` token from
//!   an `app_publishers` row;
//! - trusted access (`POST /api/auth/oidc/exchange`, audience
//!   [`deployment_audience`] — one per deployment, `oxy:<host>`), which mints a
//!   15-minute `oxy_ci_` token from a trust policy on a service account
//!   (API-tokens design §3.4).
//!
//! The layers, outermost first:
//!
//! - [`keys`] — GitHub's signing keys, cached; refreshed only on an unknown
//!   `kid`, and never per request;
//! - [`verify`] — the envelope: RS256 only, the pinned issuer, **the audience
//!   the caller requires**, `exp` and `nbf` with 30 s leeway, then the `jti`
//!   burn;
//! - [`jti`] — the single-use store, and its sweep;
//! - [`claims`] — the claims GitHub emits that either exchange reads;
//! - [`publisher`] — the pure decision for the legacy exchange;
//! - [`matcher`] — the pure decision for trusted access.
//!
//! The two decisions are pure functions with no network and no database, and
//! are where every match rule lives. Neither ever reads `sub`: its format has
//! changed before, and everything it encodes is in a claim of its own.

pub mod audience;
pub mod claims;
pub mod jti;
pub mod keys;
pub mod matcher;
pub mod publisher;
pub mod verify;

pub use audience::deployment_audience;
pub use claims::{
    AUDIENCE_OXY, AUDIENCE_PUBLISH, GITHUB_JWKS_URL, GITHUB_OIDC_ISSUER, GithubOidcClaims,
};
pub use keys::JwksCache;
pub use matcher::{ClaimReject, PolicyRule, match_policies};
pub use publisher::{OidcReject, PublisherConfig, machine_identity, verify_claims};
pub use verify::{OidcError, verify_token};

#[cfg(test)]
pub(crate) mod test_support;
