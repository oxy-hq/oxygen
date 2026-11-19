//! Where a registered issuer's keys come from.
//!
//! - **A `jwks_uri`** — read from the issuer's discovery document at
//!   registration, or given by the admin: one [`JwksCache`] per URL, with the
//!   same discipline as GitHub's — fetched on first use, refreshed only on an
//!   unknown `kid` and at most once every
//!   [`crate::github_oidc::keys::REFRESH_EVERY`], and a failed refresh keeps
//!   the keys already held. The URL is one an org admin's registration wrote,
//!   never one a token carries.
//! - **Inline** — the key set stored on the issuer ([`inline`]). Nothing is
//!   fetched, so an issuer Oxygen cannot reach can still be trusted; new keys
//!   are a new registration.
//!
//! This crate holds no HTTP client, so the fetch is handed in once per
//! process ([`install_fetcher`]): the server's fetches only public HTTPS
//! addresses (`oxy_shared::http::public_only`), and a test's answers from a
//! fixed set. With none installed, every URL's cache is empty and every token
//! from such an issuer is refused as unverifiable.
//!
//! The map is bounded at [`MAX_CACHED_ISSUERS`]: past that it is emptied and
//! refills on use, so registering issuers cannot grow it without limit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use jsonwebtoken::jwk::JwkSet;
use serde_json::Value;

use crate::github_oidc::JwksCache;
use crate::github_oidc::keys::FetchFuture;

/// The most issuers whose keys are held at once.
pub const MAX_CACHED_ISSUERS: usize = 1024;

/// Read the key set at a `jwks_uri`.
pub type FetchKeys = Arc<dyn Fn(String) -> FetchFuture + Send + Sync>;

static FETCHER: OnceLock<FetchKeys> = OnceLock::new();
static CACHES: OnceLock<Mutex<HashMap<String, Arc<JwksCache>>>> = OnceLock::new();

/// Install how this process reads a key set. First install wins: `false`
/// means one was already in place and this one was dropped.
pub fn install_fetcher(fetch: FetchKeys) -> bool {
    FETCHER.set(fetch).is_ok()
}

fn no_fetch() -> FetchFuture {
    Box::pin(async { Err(()) })
}

fn new_cache(jwks_uri: &str) -> JwksCache {
    let uri = jwks_uri.to_string();
    match FETCHER.get() {
        Some(fetch) => {
            let fetch = fetch.clone();
            JwksCache::new(move || fetch(uri.clone()))
        }
        None => JwksCache::new(no_fetch),
    }
}

/// The key cache for the issuer whose keys are at `jwks_uri`.
pub fn keys_for(jwks_uri: &str) -> Arc<JwksCache> {
    let caches = CACHES.get_or_init(|| Mutex::new(HashMap::new()));
    // A poisoned map holds nothing a panic could have half-written: every
    // write is one insert or one clear.
    let mut caches = caches.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cache) = caches.get(jwks_uri) {
        return cache.clone();
    }
    if caches.len() >= MAX_CACHED_ISSUERS {
        caches.clear();
    }
    let cache = Arc::new(new_cache(jwks_uri));
    caches.insert(jwks_uri.to_string(), cache.clone());
    cache
}

/// Drop the cache for `jwks_uri`: its issuer was deleted, and a later
/// registration of the same address starts from fresh keys.
pub fn forget(jwks_uri: &str) {
    if let Some(caches) = CACHES.get() {
        caches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(jwks_uri);
    }
}

/// The keys an issuer registered with its key set inline: exactly those, and
/// never a fetch. `None` for a stored value that is not a key set, which
/// verifies nothing.
pub fn inline(jwks: &Value) -> Option<JwksCache> {
    serde_json::from_value::<JwkSet>(jwks.clone())
        .ok()
        .map(JwksCache::fixed)
}
