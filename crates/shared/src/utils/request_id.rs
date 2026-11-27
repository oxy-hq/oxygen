//! `x-oxy-request-id` — the wire name and the reader.
//!
//! ## Why this lives in `oxy-shared` and not beside the middleware
//!
//! The middleware that *mints* the id is `oxy-app`'s
//! `server::api::middlewares::request_id`. But the custom-apps surface needs to
//! *read* it, and `crates/app/tests/custom_apps/custom_apps_boundary.rs` bans a
//! `crate::` back-edge from that surface into `server::api::middlewares` — a
//! seam deleted on purpose in 2026-07 with an explicit "do not re-add".
//!
//! The sanctioned answer, and the precedent that test records for
//! `custom_app_url`, is to move the shared piece *down* into a crate both sides
//! already depend on. So the name and the parse live here; the minting policy
//! stays in the middleware, where it belongs.

use ::http::HeaderMap;
use uuid::Uuid;

/// Wire name, on the request (for handlers), the response (for callers), and
/// the internal proxy hop.
pub const HEADER: &str = "x-oxy-request-id";

/// Read the id a request carries, if it is a well-formed UUID.
///
/// A malformed value reads as absent rather than being passed through: this is
/// a join key, and a value that cannot be a key is worse than no key at all.
pub fn from_headers(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_well_formed_id() {
        let mut headers = HeaderMap::new();
        let id = Uuid::new_v4();
        headers.insert(HEADER, id.to_string().parse().unwrap());
        assert_eq!(from_headers(&headers), Some(id));
    }

    #[test]
    fn a_malformed_id_reads_as_absent() {
        let mut headers = HeaderMap::new();
        headers.insert(HEADER, "not-a-uuid".parse().unwrap());
        assert_eq!(from_headers(&headers), None);
        assert_eq!(from_headers(&HeaderMap::new()), None);
    }
}
