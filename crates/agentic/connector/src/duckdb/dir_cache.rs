//! Process-wide cache of the DuckDB databases built over a directory of files.
//!
//! [`DuckDbConnector::from_directory`] registers every CSV/Parquet file as a
//! view and `DESCRIBE`s it. DuckDB binds a view when it is created, binding
//! `read_csv_auto` sniffs the file, and each file is bound four times (its
//! view, the full-name alias, the schema-qualified alias, the `DESCRIBE`).
//! Runs build their connectors fresh, so every analytics run against an
//! unchanged directory paid all of that again before its first event.
//!
//! This cache builds the database once per `(directory, load strategy)` and
//! file fingerprint, and hands every connector its own `try_clone()` of it:
//! a separate connection, so concurrent runs never queue on one mutex, over
//! the shared catalog. The files are registered in the database's catalog
//! rather than as `TEMP` objects because temp objects belong to the
//! connection that created them and a clone would not see them. Per-session
//! state (`file_search_path`, the ICU extension) is applied to each clone.
//!
//! **Freshness.** The fingerprint is every candidate file's name, size and
//! modification time, taken *before* the build reads the files. Adding,
//! removing or rewriting a file changes it, and the next lookup rebuilds; a
//! file that changes during a build leaves an entry that is newer than its
//! fingerprint, which the next lookup also rebuilds. A `Materialized` table
//! is a copy, and the same fingerprint keeps it fresh.
//!
//! **Isolation.** A clone only ever reads the shared catalog: before a
//! statement that may write, its connector moves onto a private database
//! ([`super::session::Sessions`]). That build runs on the blocking pool,
//! once per connector, under no lock another statement waits on. A caller
//! that writes before it reads — pre-aggregation — skips the cache instead
//! (`DuckDbConfig::reuse_cached_directory`): the clone would buy it nothing.
//!
//! **Bounds.** At most [`CAPACITY`] entries, least recently used evicted
//! first, and one entry per `(directory, strategy)` — a rebuild replaces its
//! stale entry. Each entry is two DuckDB databases: the shared one, on at
//! most [`SHARED_THREADS`] workers, and the single-threaded parse-check
//! shadow. An evicted database stays alive until
//! its last clone drops (duckdb-rs closes a database with its last
//! connection); a private copy lives as long as its connector.
//!
//! **Builds.** Two builds of one key never run at once (DuckDB concurrent
//! init has crashed the process before); a caller that waited on another's
//! build takes its result. A failed build is not cached.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Instant, UNIX_EPOCH},
};

use duckdb::Connection;

use super::parse_check::Shadow;
use super::{
    DuckDbConnector, LoadStrategy, TableInfo,
    loading::{Catalog, apply_session_setup, bound_threads, directory_candidates, open_directory},
};
use crate::connector::ConnectorError;

/// Most directory databases the process keeps warm.
pub(super) const CAPACITY: usize = 16;

/// Most worker threads a shared database runs on. DuckDB starts one per core
/// for every database it opens and keeps them for the database's life, which
/// here is the process's: [`CAPACITY`] core-sized pools would idle as
/// `CAPACITY × cores` threads. Every clone of an entry queries on its pool,
/// so it stays wide enough for a few runs at once.
pub(super) const SHARED_THREADS: usize = 4;

/// The process-wide cache [`DuckDbConnector::from_directory_cached`] reads.
pub(super) fn global() -> &'static DirCache {
    static CACHE: OnceLock<DirCache> = OnceLock::new();
    CACHE.get_or_init(|| DirCache::new(CAPACITY))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    dir: PathBuf,
    strategy: LoadStrategy,
}

/// `(file name, size, mtime seconds, mtime nanos)` per candidate file,
/// sorted.
type Fingerprint = Vec<(PathBuf, u64, u64, u32)>;

/// One cached database: the primary connection every clone is taken from,
/// and what was registered in it.
pub(super) struct SharedDb {
    /// Never queried — only cloned. `std` mutex: `try_clone` is quick and
    /// nobody awaits while holding it.
    primary: Mutex<Connection>,
    dir: PathBuf,
    strategy: LoadStrategy,
    loaded_tables: Vec<TableInfo>,
    /// What decides, with DuckDB's own parser, that a clone's statement only
    /// reads. `None` when it could not be built: then nothing is trusted to
    /// read and every write-capable call moves to a private copy.
    shadow: Option<Shadow>,
    /// Private copies opened from this entry, and how long each waits first:
    /// what a test needs to see a slow build.
    #[cfg(test)]
    private_builds: AtomicUsize,
    #[cfg(test)]
    private_build_delay: std::time::Duration,
}

impl SharedDb {
    fn build(dir: &Path, strategy: LoadStrategy) -> Result<Self, ConnectorError> {
        let (primary, loaded_tables) = open_directory(dir, strategy, Catalog::Database)?;
        bound_threads(&primary, SHARED_THREADS, dir);
        let shadow = Shadow::build(&loaded_tables, dir);
        if shadow.is_none() {
            tracing::warn!(
                dir = %dir.display(),
                "DuckDB: could not build the parse-check shadow; every statement on this \
                 directory's connectors runs on a private copy"
            );
        }
        Ok(Self {
            primary: Mutex::new(primary),
            dir: dir.to_path_buf(),
            strategy,
            loaded_tables,
            shadow,
            #[cfg(test)]
            private_builds: AtomicUsize::new(0),
            #[cfg(test)]
            private_build_delay: std::time::Duration::ZERO,
        })
    }

    /// Whether DuckDB's parser reads every statement of `sql` as a `SELECT`
    /// (`parse_check`). The guarantee behind a clone running it on the shared
    /// catalog; the lexer in `read_only` is only the fast route there.
    pub(super) fn only_reads(&self, sql: &str) -> bool {
        self.shadow.as_ref().is_some_and(|s| s.only_selects(sql))
    }

    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    /// A connector over a fresh clone of this database.
    fn connector(self: &Arc<Self>) -> Result<DuckDbConnector, ConnectorError> {
        let conn = self
            .primary
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_clone()
            .map_err(|e| ConnectorError::ConnectionError(format!("DuckDB try_clone: {e}")))?;
        apply_session_setup(&conn, &self.dir);
        Ok(DuckDbConnector::from_shared(
            conn,
            Arc::clone(self),
            self.loaded_tables.clone(),
        ))
    }

    /// A database over the same directory that no other connector reads —
    /// what [`DuckDbConnector::from_directory`] would build now.
    pub(super) fn open_private(&self) -> Result<Connection, ConnectorError> {
        #[cfg(test)]
        {
            self.private_builds.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(self.private_build_delay);
        }
        open_directory(&self.dir, self.strategy, Catalog::Temp).map(|(conn, _)| conn)
    }
}

struct Slot {
    fingerprint: Fingerprint,
    entry: Arc<SharedDb>,
    last_used: u64,
}

pub(super) struct DirCache {
    capacity: usize,
    #[cfg(test)]
    private_build_delay: std::time::Duration,
    slots: Mutex<HashMap<Key, Slot>>,
    /// One lock per key being built, so a key's builds run one at a time.
    /// An entry is removed once nobody holds or waits on it.
    build_locks: Mutex<HashMap<Key, Arc<Mutex<()>>>>,
    tick: AtomicU64,
    builds: AtomicUsize,
}

impl DirCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            #[cfg(test)]
            private_build_delay: std::time::Duration::ZERO,
            slots: Mutex::new(HashMap::new()),
            build_locks: Mutex::new(HashMap::new()),
            tick: AtomicU64::new(0),
            builds: AtomicUsize::new(0),
        }
    }

    /// A connector over `dir`'s files, sharing a cached database when the
    /// files are unchanged since it was built.
    ///
    /// Falls back to an uncached [`DuckDbConnector::from_directory`] — same
    /// result, same errors — when the directory cannot be canonicalized or a
    /// file cannot be stat'ed, since then there is no key to trust.
    pub(super) fn connector(
        &self,
        dir: &Path,
        strategy: LoadStrategy,
    ) -> Result<DuckDbConnector, ConnectorError> {
        let Ok(canonical) = dir.canonicalize() else {
            return DuckDbConnector::from_directory(dir, strategy);
        };
        let Some(fingerprint) = fingerprint(&directory_candidates(&canonical)?) else {
            return DuckDbConnector::from_directory(dir, strategy);
        };
        let key = Key {
            dir: canonical,
            strategy,
        };
        self.get_or_build(&key, fingerprint)?.connector()
    }

    fn get_or_build(
        &self,
        key: &Key,
        fingerprint: Fingerprint,
    ) -> Result<Arc<SharedDb>, ConnectorError> {
        if let Some(hit) = self.lookup(key, &fingerprint) {
            tracing::debug!(dir = %key.dir.display(), "DuckDB directory cache hit");
            return Ok(hit);
        }
        let lock = self.build_lock(key);
        let result = {
            let _building = lock.lock().unwrap_or_else(PoisonError::into_inner);
            match self.lookup(key, &fingerprint) {
                // Built by the caller we waited on.
                Some(hit) => Ok(hit),
                None => self.build(key, fingerprint),
            }
        };
        drop(lock);
        self.release_build_lock(key);
        result
    }

    fn build(&self, key: &Key, fingerprint: Fingerprint) -> Result<Arc<SharedDb>, ConnectorError> {
        // Free the stale entry's database before building its replacement.
        let stale = self.slots().remove(key);
        drop(stale);
        self.builds.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        #[allow(unused_mut)]
        let mut entry = SharedDb::build(&key.dir, key.strategy)?;
        #[cfg(test)]
        {
            entry.private_build_delay = self.private_build_delay;
        }
        let entry = Arc::new(entry);
        tracing::info!(
            dir = %key.dir.display(),
            files = entry.loaded_tables.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "DuckDB directory database built and cached"
        );
        self.insert(key.clone(), fingerprint, Arc::clone(&entry));
        Ok(entry)
    }

    fn lookup(&self, key: &Key, fingerprint: &Fingerprint) -> Option<Arc<SharedDb>> {
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        let mut slots = self.slots();
        let slot = slots.get_mut(key)?;
        if &slot.fingerprint != fingerprint {
            return None;
        }
        slot.last_used = tick;
        Some(Arc::clone(&slot.entry))
    }

    fn insert(&self, key: Key, fingerprint: Fingerprint, entry: Arc<SharedDb>) {
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        let mut slots = self.slots();
        let evicted = if !slots.contains_key(&key) && slots.len() >= self.capacity {
            let oldest = slots
                .iter()
                .min_by_key(|(_, slot)| slot.last_used)
                .map(|(k, _)| k.clone());
            oldest.and_then(|k| slots.remove(&k))
        } else {
            None
        };
        let replaced = slots.insert(
            key,
            Slot {
                fingerprint,
                entry,
                last_used: tick,
            },
        );
        drop(slots);
        // Dropped outside the map lock: closing a database can take a while.
        drop((evicted, replaced));
    }

    fn build_lock(&self, key: &Key) -> Arc<Mutex<()>> {
        let mut locks = self
            .build_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(locks.entry(key.clone()).or_default())
    }

    /// Drop `key`'s build lock once the map holds the only reference. Sound
    /// because clones are only minted under the map lock held here: a
    /// waiter's clone keeps the count above one.
    fn release_build_lock(&self, key: &Key) {
        let mut locks = self
            .build_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if locks
            .get(key)
            .is_some_and(|lock| Arc::strong_count(lock) == 1)
        {
            locks.remove(key);
        }
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every private copy opened from this cache's entries sleeps `delay`
    /// first.
    #[cfg(test)]
    pub(super) fn with_private_build_delay(mut self, delay: std::time::Duration) -> Self {
        self.private_build_delay = delay;
        self
    }

    /// The threads every parse check on this cache's live entries ran on.
    #[cfg(test)]
    pub(super) fn parse_check_threads(&self) -> Vec<std::thread::ThreadId> {
        self.slots()
            .values()
            .filter_map(|slot| slot.entry.shadow.as_ref())
            .flat_map(|shadow| shadow.checked_on())
            .collect()
    }

    /// Private copies opened from this cache's live entries.
    #[cfg(test)]
    pub(super) fn private_builds(&self) -> usize {
        self.slots()
            .values()
            .map(|slot| slot.entry.private_builds.load(Ordering::Relaxed))
            .sum()
    }

    #[cfg(test)]
    pub(super) fn builds(&self) -> usize {
        self.builds.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.slots().len()
    }

    #[cfg(test)]
    pub(super) fn build_lock_count(&self) -> usize {
        self.build_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// `None` when a file cannot be stat'ed or has no modification time: then a
/// rewrite could go unnoticed, so the caller must not cache.
fn fingerprint(files: &[PathBuf]) -> Option<Fingerprint> {
    let mut out = Vec::with_capacity(files.len());
    for path in files {
        let meta = std::fs::metadata(path).ok()?;
        let since_epoch = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        out.push((
            path.clone(),
            meta.len(),
            since_epoch.as_secs(),
            since_epoch.subsec_nanos(),
        ));
    }
    out.sort();
    Some(out)
}

#[cfg(test)]
#[path = "dir_cache_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "dir_cache_instance_tests.rs"]
mod instance_tests;

#[cfg(test)]
#[path = "dir_cache_preagg_tests.rs"]
mod preagg_tests;
