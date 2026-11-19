//! Tokens from an OIDC issuer an org registered: the verifier behind trusted
//! access for any workload that holds one (API-tokens design §3.4, *Any OIDC
//! issuer*) — the JWT AWS STS signs for a role (IAM outbound identity
//! federation), a GitLab CI job's `id_token`, any other workload identity's.
//!
//! The exchange (`POST /api/auth/oidc/exchange`) trades such a token for a
//! 15-minute `oxy_ci_` token acting as an Oxygen service account, exactly as
//! it does a GitHub Actions token. What differs is who signed it: not one
//! issuer pinned in code, but one an org admin registered (`oidc_issuers`),
//! and what it must say: not GitHub's columns, but the claim conditions of a
//! trust policy (`oidc_issuer_trust_policies`).
//!
//! The layers, outermost first:
//!
//! - [`verify`] — the envelope: RS256 only, the registered issuer (a trailing
//!   `/` ignored), **this deployment's audience and nothing else beside it**,
//!   `exp`/`nbf` with leeway, `iat` not in the future, `sub` present, and a
//!   lifetime of at most [`verify::MAX_LIFETIME_SECONDS`];
//! - [`keys`] — where the issuer's keys come from: a `jwks_uri` (read from its
//!   discovery document at registration, or given), cached per URL and
//!   refreshed only on an unknown `kid`; or the key set stored inline;
//! - [`claims`] — the verified token, and what is recorded;
//! - [`conditions`] — the pure decision: which policy's conditions the claims
//!   meet. Exactly one, or the exchange is refused.
//!
//! No layer reads which settings-form preset registered the issuer.
//!
//! **Replay.** Each issuer says whether its tokens are single use
//! (`jti_single_use`, on by default). When they are, a token must carry a
//! `jti`, which is spent at the first exchange, so a token mints at most one
//! credential. When they are not — an issuer that hands one token out more
//! than once — what bounds a replay is that the token is good only at this
//! deployment (its audience, alone), only until its `exp`, and only if that
//! `exp` is at most an hour after its `iat`. Either way the `jti` and the
//! claims are recorded on the minted token, so every exchange of one token is
//! visible as such.

pub mod claims;
pub mod conditions;
pub mod keys;
pub mod verify;

pub use claims::VerifiedToken;
pub use conditions::{Condition, Pick, pick};
pub use verify::{IssuerTokenError, normalized_issuer, unverified_issuer, verify_token};

#[cfg(test)]
mod tests;
