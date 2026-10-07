//! Whose session cookie a request carries — read, never renewed.
//!
//! `GET /auth/session` answers "is there a session cookie" too, but it is a
//! hydration endpoint: it mints a fresh thirty-day token and cookie and writes
//! `last_login_at`. That is right for a browser arriving on a new origin and
//! wrong for a check a kiosk repeats every minute — it would slide an admin's
//! session forever and stretch a crew member's twelve-hour shift session
//! (`frontline::SHIFT_HOURS`) to thirty days. This module decodes and nothing
//! else.

use axum::http::HeaderMap;
use jsonwebtoken::{Validation, decode};
use oxy_auth::session_key::{self, Purpose};

use super::dto::Claims;

/// The user id (`sub`) of the `oxy_session` cookie on this request, when that
/// cookie holds a token the server would accept: signed with this deployment's
/// session key and not expired. `None` for no cookie, an empty one, a forged
/// one, an expired one — and when the key itself cannot be read, which is
/// answered as "no session" rather than as an error a probe has to handle.
///
/// The cookie alone. An `Authorization` header is ignored on purpose: the web
/// app attaches its stored bearer token to every call, and the question this
/// answers is whether the *cookie* still backs that token.
pub async fn session_cookie_user_id(headers: &HeaderMap) -> Option<String> {
    let jwt = oxy_auth::built_in::extract_session_cookie(headers)?;
    let key = session_key::decoding_key(Purpose::Session).await.ok()?;
    decode::<Claims>(&jwt, &key, &Validation::default())
        .ok()
        .map(|data| data.claims.sub)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use chrono::Utc;
    use jsonwebtoken::{EncodingKey, Header, encode};

    /// This process's session key, with a root fixed so no database is read.
    async fn session_key_bytes() -> [u8; 32] {
        session_key::install_root_for_tests([7; 32]);
        session_key::key(Purpose::Session).await.expect("a key")
    }

    fn jwt(sub: &str, key: &[u8], exp_offset_secs: i64) -> String {
        let now = Utc::now().timestamp();
        encode(
            &Header::default(),
            &Claims {
                sub: sub.to_string(),
                email: String::new(),
                exp: (now + exp_offset_secs) as usize,
                iat: now as usize,
            },
            &EncodingKey::from_secret(key),
        )
        .expect("encode")
    }

    fn headers(pairs: &[(&'static str, String)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[tokio::test]
    async fn a_valid_session_cookie_names_its_user() {
        let tok = jwt("user-a", &session_key_bytes().await, 3600);
        let h = headers(&[("cookie", format!("oxy_kiosk=k.s; oxy_session={tok}"))]);
        assert_eq!(session_cookie_user_id(&h).await.as_deref(), Some("user-a"));
    }

    #[tokio::test]
    async fn the_authorization_header_is_not_the_cookie() {
        let key = session_key_bytes().await;
        let tok = jwt("user-a", &key, 3600);
        // A bearer alone — the web app after `/api/logout` cleared the cookie.
        let h = headers(&[("authorization", tok.clone())]);
        assert_eq!(session_cookie_user_id(&h).await, None);
        // Both, naming different people: the cookie's answer, not the bearer's.
        let other = jwt("user-b", &key, 3600);
        let h = headers(&[
            ("authorization", tok),
            ("cookie", format!("oxy_session={other}")),
        ]);
        assert_eq!(session_cookie_user_id(&h).await.as_deref(), Some("user-b"));
    }

    #[tokio::test]
    async fn an_expired_forged_or_empty_cookie_names_nobody() {
        let key = session_key_bytes().await;
        let cookie = |value: String| headers(&[("cookie", format!("oxy_session={value}"))]);

        let expired = jwt("user-a", &key, -3600);
        assert_eq!(session_cookie_user_id(&cookie(expired)).await, None);
        // Signed with a key that is not this deployment's — the string every
        // session was once signed with among them.
        for not_ours in [&b"not-our-key"[..], &b"authentication_secret"[..]] {
            let forged = jwt("user-a", not_ours, 3600);
            assert_eq!(session_cookie_user_id(&cookie(forged)).await, None);
        }
        let empty = headers(&[("cookie", "oxy_session=; x=y".into())]);
        assert_eq!(session_cookie_user_id(&empty).await, None);
        assert_eq!(session_cookie_user_id(&HeaderMap::new()).await, None);
    }
}
