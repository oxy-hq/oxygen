//! GitHub's signing keys, cached.
//!
//! Refreshed only on an unknown `kid` (a key rotation) and never per request,
//! which would make GitHub a hard availability dependency and a self-DoS
//! vector. A refresh is attempted at most once every [`REFRESH_EVERY`],
//! whether or not the last one succeeded, so a stream of unknown-`kid` tokens
//! — or GitHub being down — cannot turn into a fetch storm. A failed refresh
//! leaves the keys already held in place.
//!
//! This crate holds no HTTP client, so the fetch is handed in: the server
//! passes the one that reads [`super::GITHUB_JWKS_URL`], and a test passes a
//! fixed set.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::JwkSet;
use tokio::sync::RwLock;

use super::verify::OidcError;

/// The least time between two refresh attempts.
pub const REFRESH_EVERY: Duration = Duration::from_secs(30);

/// One attempt to read the key set. `Err` carries nothing: every failure is
/// the same "unavailable" to the caller, and the fetcher logs its own reason.
pub type FetchFuture = Pin<Box<dyn Future<Output = Result<JwkSet, ()>> + Send>>;
type Fetch = Box<dyn Fn() -> FetchFuture + Send + Sync>;

#[derive(Default)]
struct State {
    keys: Option<JwkSet>,
    /// When a refresh was last *attempted*.
    attempted_at: Option<Instant>,
}

pub struct JwksCache {
    fetch: Fetch,
    state: RwLock<State>,
}

impl JwksCache {
    /// A cache that reads its keys through `fetch`, on first use.
    pub fn new(fetch: impl Fn() -> FetchFuture + Send + Sync + 'static) -> Self {
        Self {
            fetch: Box::new(fetch),
            state: RwLock::new(State::default()),
        }
    }

    /// A cache that holds exactly `keys` and never reads any others.
    pub fn fixed(keys: JwkSet) -> Self {
        Self {
            fetch: Box::new(|| Box::pin(async { Err(()) })),
            state: RwLock::new(State {
                keys: Some(keys),
                attempted_at: None,
            }),
        }
    }

    async fn lookup(&self, kid: &str) -> Option<Result<DecodingKey, OidcError>> {
        let state = self.state.read().await;
        let jwk = state.keys.as_ref()?.find(kid)?;
        Some(DecodingKey::from_jwk(jwk).map_err(|_| OidcError::UnknownKey))
    }

    /// Whether a refresh may be attempted now, claiming the attempt if so.
    async fn claim_refresh(&self) -> bool {
        let mut state = self.state.write().await;
        let due = state
            .attempted_at
            .is_none_or(|at| at.elapsed() > REFRESH_EVERY);
        if due {
            state.attempted_at = Some(Instant::now());
        }
        due
    }

    /// The decoding key for `kid`, refreshing once on a miss.
    pub async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, OidcError> {
        if let Some(found) = self.lookup(kid).await {
            return found;
        }
        if self.claim_refresh().await {
            match (self.fetch)().await {
                Ok(keys) => self.state.write().await.keys = Some(keys),
                Err(()) => tracing::warn!("github oidc: the JWKS refresh failed"),
            }
        }
        match self.lookup(kid).await {
            Some(found) => found,
            None if self.state.read().await.keys.is_none() => Err(OidcError::JwksUnavailable),
            None => Err(OidcError::UnknownKey),
        }
    }
}

static GLOBAL: OnceLock<JwksCache> = OnceLock::new();

/// The process's cache, built by `init` the first time anything asks.
pub fn global_or(init: impl FnOnce() -> JwksCache) -> &'static JwksCache {
    GLOBAL.get_or_init(init)
}

/// Install `cache` as the process's cache. First install wins: `false` means
/// one was already in place and this one was dropped.
pub fn install(cache: JwksCache) -> bool {
    GLOBAL.set(cache).is_ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::github_oidc::test_support::{TEST_KID, jwks};

    fn counting(fetches: Arc<AtomicUsize>, result: Result<JwkSet, ()>) -> JwksCache {
        JwksCache::new(move || {
            fetches.fetch_add(1, Ordering::SeqCst);
            let result = result.clone();
            Box::pin(async move { result })
        })
    }

    #[tokio::test]
    async fn the_keys_are_fetched_once_and_then_served_from_the_cache() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let cache = counting(fetches.clone(), Ok(jwks()));
        for _ in 0..3 {
            assert!(cache.decoding_key(TEST_KID).await.is_ok());
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_unknown_kid_refreshes_at_most_once_per_window() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let cache = counting(fetches.clone(), Ok(jwks()));
        for _ in 0..5 {
            assert!(matches!(
                cache.decoding_key("rotated-away").await,
                Err(OidcError::UnknownKey)
            ));
        }
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            1,
            "a stream of unknown kids is one fetch, not five"
        );
    }

    #[tokio::test]
    async fn a_failed_fetch_is_not_retried_within_the_window() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let cache = counting(fetches.clone(), Err(()));
        for _ in 0..4 {
            assert!(matches!(
                cache.decoding_key(TEST_KID).await,
                Err(OidcError::JwksUnavailable)
            ));
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_fixed_cache_holds_its_keys_and_reads_no_others() {
        let cache = JwksCache::fixed(jwks());
        assert!(cache.decoding_key(TEST_KID).await.is_ok());
        assert!(matches!(
            cache.decoding_key("some-other-key").await,
            Err(OidcError::UnknownKey)
        ));
        // The miss did not cost it the keys it holds.
        assert!(cache.decoding_key(TEST_KID).await.is_ok());
    }
}
