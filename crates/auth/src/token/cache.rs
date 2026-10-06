//! In-process cache of resolved credentials, and the `last_used_at` throttle.
//!
//! **≤30 s.** A cached credential skips the token lookup, so a revocation on
//! another pod takes effect here within the TTL (design §4.2). A revocation on
//! *this* pod invalidates immediately ([`invalidate_token`]). Only successes are
//! cached; a refused token is looked up every time, so revoking cannot be
//! outrun by a stale negative entry and a newly extended key works at once.
//!
//! Bounded: past [`MAX_ENTRIES`] the map is cleared rather than evicted
//! piecemeal. That costs one lookup per active token, which is exactly what an
//! uncached request costs, and keeps the code obviously correct.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::credential::CredentialContext;
use crate::types::Identity;

/// How long a resolved credential is trusted without a lookup.
pub const TTL: Duration = Duration::from_secs(30);
/// How often one token's `last_used_at` is written, per process. Matches the
/// SQL guard, so the write this skips is one the database would skip anyway.
pub const TOUCH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const MAX_ENTRIES: usize = 10_000;

#[derive(Clone)]
struct Entry {
    identity: Identity,
    credential: CredentialContext,
    expires_at: Option<DateTime<Utc>>,
    cached_at: Instant,
}

/// The cache proper, clock passed in so it is testable without sleeping.
#[derive(Default)]
pub(crate) struct CredentialCache {
    entries: HashMap<Vec<u8>, Entry>,
    touched: HashMap<Uuid, Instant>,
}

impl CredentialCache {
    pub(crate) fn get(
        &mut self,
        token_hash: &[u8],
        now: Instant,
        wall: DateTime<Utc>,
    ) -> Option<(Identity, CredentialContext)> {
        let entry = self.entries.get(token_hash)?;
        let stale = now.duration_since(entry.cached_at) >= TTL;
        let lapsed = entry.expires_at.is_some_and(|at| at <= wall);
        if stale || lapsed {
            self.entries.remove(token_hash);
            return None;
        }
        Some((entry.identity.clone(), entry.credential.clone()))
    }

    pub(crate) fn put(
        &mut self,
        token_hash: Vec<u8>,
        identity: Identity,
        credential: CredentialContext,
        expires_at: Option<DateTime<Utc>>,
        now: Instant,
    ) {
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(
            token_hash,
            Entry {
                identity,
                credential,
                expires_at,
                cached_at: now,
            },
        );
    }

    pub(crate) fn invalidate(&mut self, token_id: Uuid) {
        self.entries
            .retain(|_, e| e.credential.token_id != token_id);
    }

    /// True when this token's `last_used_at` is due a write, recording the
    /// write as done. At most once per [`TOUCH_INTERVAL`] per process.
    pub(crate) fn claim_touch(&mut self, token_id: Uuid, now: Instant) -> bool {
        if self
            .touched
            .get(&token_id)
            .is_some_and(|at| now.duration_since(*at) < TOUCH_INTERVAL)
        {
            return false;
        }
        if self.touched.len() >= MAX_ENTRIES {
            self.touched.clear();
        }
        self.touched.insert(token_id, now);
        true
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.touched.clear();
    }
}

static CACHE: LazyLock<Mutex<CredentialCache>> =
    LazyLock::new(|| Mutex::new(CredentialCache::default()));

fn with_cache<T>(f: impl FnOnce(&mut CredentialCache) -> T) -> T {
    // A poisoned lock means a panic mid-update of a plain map; the data is
    // still a valid map, and a cache must never take authentication down.
    let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    f(&mut guard)
}

pub(crate) fn get(token_hash: &[u8]) -> Option<(Identity, CredentialContext)> {
    with_cache(|c| c.get(token_hash, Instant::now(), Utc::now()))
}

pub(crate) fn put(
    token_hash: Vec<u8>,
    identity: Identity,
    credential: CredentialContext,
    expires_at: Option<DateTime<Utc>>,
) {
    with_cache(|c| c.put(token_hash, identity, credential, expires_at, Instant::now()));
}

pub(crate) fn claim_touch(token_id: Uuid) -> bool {
    with_cache(|c| c.claim_touch(token_id, Instant::now()))
}

/// Drop any cached resolution of this token. Call after revoking or changing
/// its expiry, once the change has committed.
pub fn invalidate_token(token_id: Uuid) {
    with_cache(|c| c.invalidate(token_id));
}

/// Drop every cached credential, keeping the `last_used_at` throttle. Call
/// after a change that may alter many tokens' reach at once — an org's token
/// policy — once it has committed. Other pods follow within [`TTL`].
pub fn invalidate_all() {
    with_cache(|c| c.entries.clear());
}

/// Drop everything. For tests that change a token behind this process's back
/// (simulating another pod), and for an operator who needs it now.
pub fn clear() {
    with_cache(CredentialCache::clear);
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
