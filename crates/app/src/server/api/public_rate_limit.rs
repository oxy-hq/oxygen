//! A per-client rate limit on the public token routes (API-tokens design §8
//! Phase 5): `POST /api/auth/tokens/revoke-leaked` and
//! `POST /api/auth/oidc/exchange`. Neither has a credential in front of it,
//! so each carries its own budget per client address.
//!
//! The same token bucket the magic-link request uses (`governor`), in process:
//! a coarse brake per replica, not a shared quota — a Postgres write per
//! request to slow down a caller would cost more than what it guards. Each
//! route keeps its own buckets, so a burst of leak reports never starves a
//! CI job's exchange.
//!
//! **The client** is the address our load balancer saw connect: the *last*
//! `X-Forwarded-For` hop, read by `oxy_app_core::forwarded::client_ip`, the
//! same function audit rows and token usage record. The first hop is whatever
//! the caller wrote, so keying on it would let a caller pick a fresh bucket
//! per request. With no header (a direct connection, a test) every request
//! shares one bucket.

use std::num::NonZeroU32;
use std::sync::LazyLock;

use axum::http::HeaderMap;
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use oxy_app_core::forwarded::client_ip;

/// Requests per minute one client may make to one route, as a burst that
/// refills at one a second.
pub const PER_MINUTE: u32 = 60;

/// The public routes that are limited, each with its own buckets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PublicRoute {
    RevokeLeaked,
    OidcExchange,
}

/// One limiter: a bucket per `(route, client)`.
pub struct Limiter(DefaultKeyedRateLimiter<(PublicRoute, String)>);

impl Limiter {
    pub fn per_minute(n: u32) -> Self {
        let n = NonZeroU32::new(n).unwrap_or(NonZeroU32::MIN);
        Self(RateLimiter::keyed(Quota::per_minute(n)))
    }

    /// `None` when the request may go ahead; otherwise the whole seconds to
    /// wait (at least one).
    pub fn check(&self, route: PublicRoute, client: &str) -> Option<u64> {
        match self.0.check_key(&(route, client.to_string())) {
            Ok(()) => None,
            Err(not_until) => {
                let wait = not_until.wait_time_from(DefaultClock::default().now());
                Some(wait.as_secs().max(1))
            }
        }
    }
}

static LIMITER: LazyLock<Limiter> = LazyLock::new(|| Limiter::per_minute(PER_MINUTE));

/// The client a request is limited as. See the module docs.
pub fn client_key(headers: &HeaderMap) -> String {
    client_ip(headers).unwrap_or_else(|| "direct".to_string())
}

/// Charge one request to `route` for this request's client: `Some(seconds)`
/// when it is over budget.
pub fn check(route: PublicRoute, headers: &HeaderMap) -> Option<u64> {
    LIMITER.check(route, &client_key(headers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_past_its_budget_is_told_to_wait() {
        let limiter = Limiter::per_minute(3);
        for _ in 0..3 {
            assert_eq!(limiter.check(PublicRoute::RevokeLeaked, "1.2.3.4"), None);
        }
        let wait = limiter.check(PublicRoute::RevokeLeaked, "1.2.3.4");
        assert!(wait.is_some_and(|s| s >= 1), "{wait:?}");
    }

    #[test]
    fn each_client_and_each_route_has_its_own_bucket() {
        let limiter = Limiter::per_minute(1);
        assert_eq!(limiter.check(PublicRoute::RevokeLeaked, "a"), None);
        assert!(limiter.check(PublicRoute::RevokeLeaked, "a").is_some());
        assert_eq!(limiter.check(PublicRoute::RevokeLeaked, "b"), None);
        assert_eq!(limiter.check(PublicRoute::OidcExchange, "a"), None);
    }

    #[test]
    fn the_client_is_the_hop_the_load_balancer_appended() {
        let mut headers = HeaderMap::new();
        assert_eq!(client_key(&headers), "direct");
        headers.insert("x-forwarded-for", "6.6.6.6, 10.0.0.1".parse().unwrap());
        assert_eq!(
            client_key(&headers),
            "10.0.0.1",
            "the caller wrote the first"
        );
        headers.insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert_eq!(client_key(&headers), "203.0.113.9");
        // Sent on two lines, it is still the last entry of the last line.
        headers.insert("x-forwarded-for", "6.6.6.6".parse().unwrap());
        headers.append("x-forwarded-for", "7.7.7.7, 10.0.0.1".parse().unwrap());
        assert_eq!(client_key(&headers), "10.0.0.1");
    }

    #[test]
    fn the_shipped_budget_is_sixty_a_minute() {
        assert_eq!(PER_MINUTE, 60);
        let limiter = Limiter::per_minute(PER_MINUTE);
        for _ in 0..PER_MINUTE {
            assert_eq!(limiter.check(PublicRoute::OidcExchange, "c"), None);
        }
        assert!(limiter.check(PublicRoute::OidcExchange, "c").is_some());
    }
}
