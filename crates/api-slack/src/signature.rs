//! Slack request signature verification.
//!
//! Spec: <https://api.slack.com/authentication/verifying-requests-from-slack>
//! Signature base string is `v0:{timestamp}:{raw_body}`; HMAC-SHA256 keyed on
//! the per-env signing secret. Timestamps older than 5 minutes are rejected
//! (replay protection).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

const MAX_TIMESTAMP_SKEW_SECS: i64 = 60 * 5;

pub fn verify_request(
    signing_secret: &str,
    timestamp_header: Option<&str>,
    signature_header: Option<&str>,
    raw_body: &[u8],
    now_unix: i64,
) -> Result<(), SignatureError> {
    let ts = timestamp_header.ok_or(SignatureError::MissingHeaders)?;
    let sig = signature_header.ok_or(SignatureError::MissingHeaders)?;

    let ts_num: i64 = ts.parse().map_err(|_| SignatureError::BadTimestamp)?;
    if (now_unix - ts_num).abs() > MAX_TIMESTAMP_SKEW_SECS {
        return Err(SignatureError::Replay);
    }

    let base = format!("v0:{ts}:");
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_secret.as_bytes())
        .map_err(|_| SignatureError::BadKey)?;
    mac.update(base.as_bytes());
    mac.update(raw_body);
    let expected = mac.finalize().into_bytes();
    let expected_hex = hex::encode(expected);
    let expected_header = format!("v0={expected_hex}");

    if constant_time_eq(expected_header.as_bytes(), sig.as_bytes()) {
        Ok(())
    } else {
        Err(SignatureError::Mismatch)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for i in 0..a.len() {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

#[derive(Debug, PartialEq, Eq)]
pub enum SignatureError {
    MissingHeaders,
    BadTimestamp,
    Replay,
    BadKey,
    Mismatch,
}

impl std::fmt::Display for SignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingHeaders => "missing X-Slack-Signature or X-Slack-Request-Timestamp",
            Self::BadTimestamp => "timestamp is not an integer",
            Self::Replay => "timestamp is too old (> 5 min skew)",
            Self::BadKey => "signing key invalid",
            Self::Mismatch => "signature mismatch",
        })
    }
}
impl std::error::Error for SignatureError {}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    fn sign(secret: &str, ts: i64, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("v0:{ts}:").as_bytes());
        mac.update(body);
        format!("v0={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn valid_signature_accepted() {
        let secret = "shh";
        let ts = 1_700_000_000;
        let body = b"payload=x";
        let sig = sign(secret, ts, body);
        assert!(verify_request(secret, Some(&ts.to_string()), Some(&sig), body, ts).is_ok());
    }

    #[test]
    fn missing_headers_rejected() {
        let err = verify_request("shh", None, None, b"", 0).unwrap_err();
        assert_eq!(err, SignatureError::MissingHeaders);
    }

    #[test]
    fn replay_rejected() {
        let secret = "shh";
        let ts = 1_700_000_000;
        let sig = sign(secret, ts, b"");
        let err =
            verify_request(secret, Some(&ts.to_string()), Some(&sig), b"", ts + 600).unwrap_err();
        assert_eq!(err, SignatureError::Replay);
    }

    #[test]
    fn bad_signature_rejected() {
        let err = verify_request(
            "shh",
            Some("1700000000"),
            Some("v0=deadbeef"),
            b"",
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(err, SignatureError::Mismatch);
    }
}
