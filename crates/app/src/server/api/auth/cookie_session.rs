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
use jsonwebtoken::{DecodingKey, Validation, decode};
use oxy::config::constants::AUTHENTICATION_SECRET_KEY;

use super::dto::Claims;

/// The user id (`sub`) of the `oxy_session` cookie on this request, when that
/// cookie holds a token the server would accept: signed with our key and not
/// expired. `None` for no cookie, an empty one, a forged one, an expired one.
///
/// The cookie alone. An `Authorization` header is ignored on purpose: the web
/// app attaches its stored bearer token to every call, and the question this
/// answers is whether the *cookie* still backs that token.
pub(crate) fn session_cookie_user_id(headers: &HeaderMap) -> Option<String> {
    let jwt = oxy_auth::built_in::extract_session_cookie(headers)?;
    decode::<Claims>(
        &jwt,
        &DecodingKey::from_secret(AUTHENTICATION_SECRET_KEY.as_bytes()),
        &Validation::default(),
    )
    .ok()
    .map(|data| data.claims.sub)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use chrono::Utc;
    use jsonwebtoken::{EncodingKey, Header, encode};

    fn jwt(sub: &str, key: &str, exp_offset_secs: i64) -> String {
        let now = Utc::now().timestamp();
        encode(
            &Header::default(),
            &Claims {
                sub: sub.to_string(),
                email: String::new(),
                exp: (now + exp_offset_secs) as usize,
                iat: now as usize,
            },
            &EncodingKey::from_secret(key.as_bytes()),
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

    #[test]
    fn a_valid_session_cookie_names_its_user() {
        let tok = jwt("user-a", AUTHENTICATION_SECRET_KEY, 3600);
        let h = headers(&[("cookie", format!("oxy_kiosk=k.s; oxy_session={tok}"))]);
        assert_eq!(session_cookie_user_id(&h).as_deref(), Some("user-a"));
    }

    #[test]
    fn the_authorization_header_is_not_the_cookie() {
        let tok = jwt("user-a", AUTHENTICATION_SECRET_KEY, 3600);
        // A bearer alone — the web app after `/api/logout` cleared the cookie.
        let h = headers(&[("authorization", tok.clone())]);
        assert_eq!(session_cookie_user_id(&h), None);
        // Both, naming different people: the cookie's answer, not the bearer's.
        let other = jwt("user-b", AUTHENTICATION_SECRET_KEY, 3600);
        let h = headers(&[
            ("authorization", tok),
            ("cookie", format!("oxy_session={other}")),
        ]);
        assert_eq!(session_cookie_user_id(&h).as_deref(), Some("user-b"));
    }

    #[test]
    fn an_expired_forged_or_empty_cookie_names_nobody() {
        let expired = jwt("user-a", AUTHENTICATION_SECRET_KEY, -3600);
        assert_eq!(
            session_cookie_user_id(&headers(&[("cookie", format!("oxy_session={expired}"))])),
            None
        );
        let forged = jwt("user-a", "not-our-key", 3600);
        assert_eq!(
            session_cookie_user_id(&headers(&[("cookie", format!("oxy_session={forged}"))])),
            None
        );
        assert_eq!(
            session_cookie_user_id(&headers(&[("cookie", "oxy_session=; x=y".into())])),
            None
        );
        assert_eq!(session_cookie_user_id(&HeaderMap::new()), None);
    }
}
