//! What each app's production build declares under `oltpChanges`, kept per
//! app with the build it was read from, so the drift check reads a manifest
//! only when production's build changed.
//!
//! A build's manifest is written once, when it is published, and never again,
//! so what was read from a build stays true of it. Every pass still reads each
//! app's production pointer (`live_declared`), and an entry answers only while
//! the pointer is at the entry's build: a promote, a rollback or a promoting
//! publish moves it, and the next pass reads the new build in place of the one
//! kept. The drift check so always compares the store against the build
//! production serves.
//!
//! Process-local and bounded: at most [`MAX_APPS`] apps, the least recently
//! checked dropped first, and one build an app. A build the read did not
//! return is not kept, so the next pass asks for it again. A block that does
//! not parse is kept as declaring nothing, so it is logged once a build a
//! process reads, not every pass.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use lru::LruCache;
use oxy_custom_apps::manifest::OltpChangesConfig;
use uuid::Uuid;

/// Apps one replica relays for, with room to spare.
const MAX_APPS: usize = 4096;

/// What a build declares under `oltpChanges`: `None` for nothing.
pub(super) type Declaration = Option<Arc<OltpChangesConfig>>;

/// What [`Manifests::declared`]'s `read` returns: what each build it found
/// declares, by build id.
pub(super) type ReadBuilds = Result<HashMap<Uuid, Option<OltpChangesConfig>>, String>;

/// The process's one [`Manifests`], which every pass reads through.
pub(super) static MANIFESTS: LazyLock<Manifests> = LazyLock::new(|| Manifests::new(MAX_APPS));

/// Each app's production build as a pass last read it, and what it declares.
pub(super) struct Manifests {
    entries: Mutex<LruCache<Uuid, Entry>>,
}

struct Entry {
    build: Uuid,
    declared: Declaration,
}

impl Manifests {
    pub(super) fn new(max_apps: usize) -> Self {
        let max_apps = NonZeroUsize::new(max_apps).expect("max_apps > 0");
        Self {
            entries: Mutex::new(LruCache::new(max_apps)),
        }
    }

    /// What each of `live`'s `(app, production build)` declares. An app kept
    /// at that very build is answered from here; the others' builds are
    /// handed to `read` in one call, or none when every app is kept, and what
    /// it returns is kept. An app whose build `read` did not return is left
    /// out.
    pub(super) async fn declared<R, F>(
        &self,
        live: &[(Uuid, Uuid)],
        read: R,
    ) -> Result<HashMap<Uuid, Declaration>, String>
    where
        R: FnOnce(Vec<Uuid>) -> F,
        F: Future<Output = ReadBuilds>,
    {
        let (mut declared, stale) = self.kept(live);
        if stale.is_empty() {
            return Ok(declared);
        }
        let mut found = read(stale.iter().map(|&(_, build)| build).collect()).await?;
        let mut entries = self.lock();
        for (app, build) in stale {
            let Some(config) = found.remove(&build) else {
                continue;
            };
            let config = config.map(Arc::new);
            entries.put(
                app,
                Entry {
                    build,
                    declared: config.clone(),
                },
            );
            declared.insert(app, config);
        }
        Ok(declared)
    }

    /// What each app of `live` kept at its production build declares, and
    /// the `(app, build)` of every other.
    fn kept(&self, live: &[(Uuid, Uuid)]) -> (HashMap<Uuid, Declaration>, Vec<(Uuid, Uuid)>) {
        let mut entries = self.lock();
        let mut declared = HashMap::new();
        let mut stale = Vec::new();
        for &(app, build) in live {
            match entries.get(&app) {
                Some(entry) if entry.build == build => {
                    declared.insert(app, entry.declared.clone());
                }
                _ => stale.push((app, build)),
            }
        }
        (declared, stale)
    }

    fn lock(&self) -> MutexGuard<'_, LruCache<Uuid, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
