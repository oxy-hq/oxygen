//! A verified token from a registered OIDC issuer: the claims the exchange
//! reads, and what it records.
//!
//! Every issuer's token has the registered claims ([RFC 7519 §4.1]); the rest
//! is the issuer's own. The JWT AWS STS signs for a role, for one:
//!
//! ```json
//! {
//!   "iss": "https://abc123-def456.tokens.sts.global.api.aws",
//!   "sub": "arn:aws:iam::123456789012:role/ferret-task",
//!   "aud": "oxy:app.oxygen-hq.com",
//!   "iat": 1700000000, "exp": 1700000300,
//!   "jti": "…",
//!   "https://sts.amazonaws.com/": { "aws_account": "123456789012", "source_region": "us-west-2" }
//! }
//! ```
//!
//! Which claims a policy reads is its conditions' business
//! ([`super::conditions`]); nothing here knows a provider.
//!
//! [RFC 7519 §4.1]: https://www.rfc-editor.org/rfc/rfc7519#section-4.1

use serde_json::{Map, Value, json};

/// The most the recorded claims may be, as JSON. An AWS STS token's whole
/// claim set is well under it; past it only the top-level scalars are kept.
pub const RECORDED_MAX_BYTES: usize = 4096;
/// The longest scalar kept when the claim set is cut down.
const RECORDED_SCALAR_MAX_CHARS: usize = 256;

/// A token whose signature and envelope held.
#[derive(Clone, Debug)]
pub struct VerifiedToken {
    /// As the token carries it.
    pub iss: String,
    pub sub: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: Option<String>,
    /// Every claim, as signed.
    pub claims: Map<String, Value>,
}

impl VerifiedToken {
    /// The claim set as recorded: whole when it is small, else its top-level
    /// scalars of a readable length.
    fn bounded_claims(&self) -> Value {
        let whole = Value::Object(self.claims.clone());
        if whole.to_string().len() <= RECORDED_MAX_BYTES {
            return whole;
        }
        let scalars: Map<String, Value> = self
            .claims
            .iter()
            .filter(|(_, v)| match v {
                Value::String(s) => s.chars().count() <= RECORDED_SCALAR_MAX_CHARS,
                Value::Number(_) | Value::Bool(_) => true,
                _ => false,
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Value::Object(scalars)
    }

    /// What a minted token records, and the audit row carries. Never the JWT.
    pub fn recorded(&self) -> Value {
        json!({
            "iss": self.iss,
            "sub": self.sub,
            "jti": self.jti,
            "iat": self.iat,
            "exp": self.exp,
            "claims": self.bounded_claims(),
        })
    }
}
