use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use uuid::Uuid;

const STATE_TTL: Duration = Duration::hours(1);

/// Compute the OAuth callback `redirect_uri` base and the `state` origin suffix
/// for the bounce proxy (local multi-instance dev).
///
/// `OXY_OAUTH_REDIRECT_ORIGIN`, when set, holds the single registered proxy
/// origin. The callback `redirect_uri` then points at the proxy, and the
/// instance origin is carried in `state` as `~base64url(origin)` so the proxy
/// can forward the callback back to the instance that started the flow. Unset →
/// the instance origin is used directly with an empty suffix, exactly as before.
pub fn bounce_redirect(instance_origin: &str) -> (String, String) {
    let instance_origin = instance_origin.trim_end_matches('/');
    match std::env::var("OXY_OAUTH_REDIRECT_ORIGIN")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
    {
        Some(proxy) => {
            let suffix = format!("~{}", URL_SAFE_NO_PAD.encode(instance_origin));
            (proxy, suffix)
        }
        None => (instance_origin.to_string(), String::new()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    Oauth,
    Install,
}

impl Flow {
    fn as_str(&self) -> &'static str {
        match self {
            Flow::Oauth => "oauth",
            Flow::Install => "install",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "oauth" => Some(Flow::Oauth),
            "install" => Some(Flow::Install),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StatePayload {
    pub org_id: Uuid,
    /// The person the flow was started for. The callback is completed by
    /// whichever session GitHub's redirect lands in, so this — signed, not the
    /// session — is what says the two are the same person.
    pub user_id: Uuid,
    pub flow: Flow,
}

#[derive(Debug)]
pub enum StateError {
    MissingSecret,
    Malformed,
    BadHmac,
    Expired,
    /// A sound state, minted for someone other than the caller completing it.
    WrongUser,
}

impl From<StateError> for StatusCode {
    fn from(err: StateError) -> Self {
        match err {
            StateError::MissingSecret => StatusCode::INTERNAL_SERVER_ERROR,
            StateError::WrongUser => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

fn secret() -> Result<String, StateError> {
    std::env::var("GITHUB_STATE_SECRET").map_err(|_| StateError::MissingSecret)
}

fn sign(data: &str, key: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
    mac.update(data.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn encode_state(payload: &StatePayload) -> Result<String, StateError> {
    encode_state_with_timestamp(payload, Utc::now().timestamp())
}

pub fn encode_state_with_timestamp(payload: &StatePayload, ts: i64) -> Result<String, StateError> {
    let key = secret()?;
    let body = format!(
        "{}:{}:{}:{}",
        payload.org_id,
        payload.user_id,
        payload.flow.as_str(),
        ts
    );
    let sig = sign(&body, key.as_bytes());
    Ok(format!("{body}:{sig}"))
}

pub fn decode_state(state: &str) -> Result<StatePayload, StateError> {
    let key = secret()?;
    // Local multi-instance dev: the OAuth bounce proxy appends the instance
    // origin to `state` as `~base64url(origin)` so it knows where to forward the
    // callback. That suffix is outside the HMAC body — strip it before verifying.
    let state = state.split('~').next().unwrap_or(state);
    let parts: Vec<&str> = state.split(':').collect();
    // A state minted before it named its user has four parts, and is refused:
    // it says nothing about who may complete it.
    if parts.len() != 5 {
        return Err(StateError::Malformed);
    }
    let [org_id_s, user_id_s, flow_s, ts_s, sig_s] =
        [parts[0], parts[1], parts[2], parts[3], parts[4]];

    let body = format!("{org_id_s}:{user_id_s}:{flow_s}:{ts_s}");
    let expected = sign(&body, key.as_bytes());
    if !constant_time_eq::constant_time_eq(sig_s.as_bytes(), expected.as_bytes()) {
        return Err(StateError::BadHmac);
    }

    let ts = ts_s.parse::<i64>().map_err(|_| StateError::Malformed)?;
    let when = DateTime::<Utc>::from_timestamp(ts, 0).ok_or(StateError::Malformed)?;
    if Utc::now().signed_duration_since(when) > STATE_TTL {
        return Err(StateError::Expired);
    }

    Ok(StatePayload {
        org_id: Uuid::parse_str(org_id_s).map_err(|_| StateError::Malformed)?,
        user_id: Uuid::parse_str(user_id_s).map_err(|_| StateError::Malformed)?,
        flow: Flow::from_str(flow_s).ok_or(StateError::Malformed)?,
    })
}

/// [`decode_state`], for the caller completing the flow: a state minted for
/// anyone else is [`StateError::WrongUser`].
pub fn decode_state_for(state: &str, caller: Uuid) -> Result<StatePayload, StateError> {
    let payload = decode_state(state)?;
    if payload.user_id != caller {
        return Err(StateError::WrongUser);
    }
    Ok(payload)
}

#[cfg(test)]
pub(crate) fn sign_for_test(body: &str) -> String {
    sign(body, secret().unwrap().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn env_secret() {
        // SAFETY: tests run single-threaded per file; overriding a process env var is fine here.
        unsafe { std::env::set_var("GITHUB_STATE_SECRET", "test-secret-value-abc") };
    }

    fn payload(flow: Flow) -> StatePayload {
        StatePayload {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            flow,
        }
    }

    #[test]
    fn encode_then_decode_roundtrips() {
        env_secret();
        let minted = payload(Flow::Oauth);
        let state = encode_state(&minted).unwrap();

        let decoded = decode_state(&state).unwrap();
        assert_eq!(decoded.org_id, minted.org_id);
        assert_eq!(decoded.user_id, minted.user_id);
        assert!(matches!(decoded.flow, Flow::Oauth));
    }

    #[test]
    fn flow_install_roundtrips() {
        env_secret();
        let encoded = encode_state(&payload(Flow::Install)).unwrap();
        let decoded = decode_state(&encoded).unwrap();
        assert!(matches!(decoded.flow, Flow::Install));
    }

    #[test]
    fn bounce_proxy_origin_suffix_stripped_before_verify() {
        env_secret();
        let minted = payload(Flow::Install);
        let state = encode_state(&minted).unwrap();
        // The bounce proxy appends `~base64url(origin)` to state.
        let with_origin = format!("{state}~aHR0cDovL2xvY2FsaG9zdDo1MTc0");
        let decoded = decode_state(&with_origin).unwrap();
        assert_eq!(decoded.org_id, minted.org_id);
        assert!(matches!(decoded.flow, Flow::Install));
    }

    #[test]
    fn tampered_hmac_rejected() {
        env_secret();
        let mut encoded = encode_state(&payload(Flow::Oauth)).unwrap();
        let last = encoded.pop().unwrap();
        encoded.push(if last == 'a' { 'b' } else { 'a' });
        assert!(decode_state(&encoded).is_err());
    }

    #[test]
    fn expired_state_rejected() {
        env_secret();
        let old_ts = (chrono::Utc::now() - chrono::Duration::hours(2)).timestamp();
        let encoded = encode_state_with_timestamp(&payload(Flow::Oauth), old_ts).unwrap();
        assert!(decode_state(&encoded).is_err());
    }

    #[test]
    fn unknown_flow_rejected() {
        env_secret();
        let raw = format!(
            "{}:{}:{}:{}",
            Uuid::new_v4(),
            Uuid::new_v4(),
            "unknown",
            chrono::Utc::now().timestamp()
        );
        let hmac = sign_for_test(&raw);
        let tampered = format!("{}:{}", raw, hmac);
        assert!(decode_state(&tampered).is_err());
    }

    #[test]
    fn a_state_minted_for_another_user_is_refused() {
        env_secret();
        let minted = payload(Flow::Install);
        let state = encode_state(&minted).unwrap();

        assert!(matches!(
            decode_state_for(&state, Uuid::new_v4()),
            Err(StateError::WrongUser)
        ));
        assert_eq!(
            decode_state_for(&state, minted.user_id).unwrap().org_id,
            minted.org_id
        );
    }

    /// A correctly signed state from before the user was signed in is not
    /// read as anyone's.
    #[test]
    fn a_state_that_names_no_user_is_refused() {
        env_secret();
        let raw = format!(
            "{}:{}:{}",
            Uuid::new_v4(),
            "install",
            chrono::Utc::now().timestamp()
        );
        let signed = format!("{raw}:{}", sign_for_test(&raw));
        assert!(matches!(decode_state(&signed), Err(StateError::Malformed)));
    }

    /// A refused state is never a `401`: the web app signs the person out on
    /// one, and a stale state only needs the flow started again.
    #[test]
    fn a_refused_state_is_never_a_sign_out() {
        for (err, status) in [
            (StateError::Malformed, StatusCode::BAD_REQUEST),
            (StateError::BadHmac, StatusCode::BAD_REQUEST),
            (StateError::Expired, StatusCode::BAD_REQUEST),
            (StateError::WrongUser, StatusCode::FORBIDDEN),
            (StateError::MissingSecret, StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            assert_eq!(StatusCode::from(err), status);
        }
    }
}
