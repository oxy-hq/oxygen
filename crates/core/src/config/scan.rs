//! Postgres-backed FS materialisers for callers that walk a directory
//! / file path: airlayer (semantic scan) and the metric-monitoring
//! crate (`.monitor.yml`).
//!
//! `airlayer` (and the local semantic engine on top of it) consumes a
//! filesystem directory of `.view.yml` + `.topic.yml` files and parses
//! them itself. We don't want to fork that loader to take Postgres
//! rows directly — too much surface and the loader semantics are
//! intentionally FS-coupled (glob ordering, dialect-aware shims, etc.).
//!
//! Instead, when the compile boundary is wired on, we *materialise*
//! the compiled rows into a temporary directory laid out the way
//! airlayer expects and hand THAT directory back as the scan path.
//!
//! Cost: the writes were never the expensive part. The read in front of them
//! was — two boundary queries, then one S3 GET per row whenever
//! `OXY_COMPILE_BLOB_S3_BUCKET` is set (every serve replica sets it), issued one
//! after another, on every semantic request, before the engine cache was even
//! consulted. A revision never changes once written, so the materialised
//! directory is now cached per `(workspace_id, revision_id)` (`scan_cache`),
//! and the one materialise a revision still pays resolves its bodies
//! concurrently. The tempdir therefore outlives the request: it is deleted
//! once the cache has evicted it AND the last request holding it has finished.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use lru::LruCache;
use oxy_shared::errors::OxyError;
use tempfile::TempDir;
use uuid::Uuid;

use super::artifacts::{ArtifactError, CompiledArtifact, VerifiedQueryEntry};
use super::manager::{ConfigManager, DiskSlot, Origin};
use super::storage::ConfigStorage;

/// A directory `airlayer` can scan, whatever the manager reads from.
///
/// On `Origin::Disk` this is the working copy itself — no bytes are copied. On
/// `Origin::Compiled` the rows are written into a tempdir laid out the way
/// airlayer expects:
///
/// ```text
/// <tmp>/semantics/views/<file_path>
/// <tmp>/semantics/topics/<file_path>
/// ```
///
/// The guard MUST be held until parsing finishes. A materialised tempdir is
/// shared — the scan cache holds one clone and every request reading that
/// revision holds another — and is deleted when the last clone drops.
#[derive(Clone)]
pub enum ScanDir {
    Real(PathBuf),
    Materialised {
        _dir: Arc<TempDir>,
        scan_path: PathBuf,
    },
}

impl ScanDir {
    pub fn path(&self) -> &std::path::Path {
        match self {
            ScanDir::Real(p) => p,
            ScanDir::Materialised { scan_path, .. } => scan_path,
        }
    }

    /// Whether this scan read the compiled revision rather than the working
    /// copy.
    ///
    /// A node can hold a working copy AND be pinned to a revision, so
    /// `ConfigManager::revision_id()` cannot answer this — it reports the pin,
    /// not the source. Anything caching by layer identity has to ask here.
    pub fn is_materialised(&self) -> bool {
        matches!(self, ScanDir::Materialised { .. })
    }
}

/// The scan directory for whichever source this manager reads.
///
/// Replaces a `materialise -> Option` that only knew the compiled arm and left
/// each caller to work out the other one.
pub async fn scan_dir<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<ScanDir, ArtifactError> {
    scan_once(config_manager.origin(), read_scan_dir(config_manager)).await
}

/// What [`scan_dir`] reads on a cache miss.
async fn read_scan_dir<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<ScanDir, ArtifactError> {
    let (Some(views), Some(topics)) = (
        config_manager.compiled_semantic_views().await?,
        config_manager.compiled_semantic_topics().await?,
    ) else {
        // Reading the working copy — the directory is already on disk.
        return Ok(ScanDir::Real(config_manager.semantics_scan_dir()?));
    };

    if views.is_empty() && topics.is_empty() {
        // A promoted revision with no semantic files.
        //
        // On a node that HAS a working copy, prefer it: the revision may be
        // older than the files on disk, and an empty scan dir would hide them
        // — airlayer reads an empty directory as "scanned, found none", which
        // is absent-reported-as-empty in the other direction.
        //
        // On a node that has none, that reasoning inverts. Zero semantic rows
        // in the promoted revision IS the whole answer, so an empty scan dir
        // states it rather than overstating it. Erroring here instead made a
        // legitimate workspace — compiled, promoted, no semantic model yet — a
        // permanently retryable 503 on every replica: neither input changes on
        // a retry, so the caller polls forever.
        return scan_dir_for_empty_revision(config_manager);
    }

    materialise(config_manager.origin(), &views, &topics).await
}

/// The scan directory for the COMPILED revision and nothing else.
///
/// [`scan_dir`] falls back to this node's working copy twice — when the manager
/// reads from disk at all, and when a promoted revision has no semantic rows
/// (where preferring the files on disk avoids reporting absence as emptiness).
/// Both are right for a reader that SERVES what the IDE is editing.
///
/// Both are wrong for a reader that JUDGES the workspace. The workspace-health
/// eval is the one such reader: it can land on a node that holds a working
/// copy, and that working copy holds whatever someone is editing right now. A
/// half-written view there would page the on-call about a workspace whose
/// promoted revision — and so everything actually being served — is fine. So
/// health asks here, and takes `NoSource` as the answer when nothing is
/// compiled: no opinion beats an opinion about the wrong files.
pub async fn compiled_scan_dir<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<ScanDir, ArtifactError> {
    // The cache only ever holds a compiled revision's materialisation, which is
    // exactly what this resolver would build — so a hit is safe here too.
    scan_once(
        config_manager.origin(),
        read_compiled_scan_dir(config_manager),
    )
    .await
}

/// What [`compiled_scan_dir`] reads on a cache miss.
async fn read_compiled_scan_dir<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<ScanDir, ArtifactError> {
    let (Some(views), Some(topics)) = (
        config_manager.compiled_semantic_views().await?,
        config_manager.compiled_semantic_topics().await?,
    ) else {
        return Err(ArtifactError::NoSource);
    };

    // A promoted revision with zero semantic rows is a complete answer, and the
    // only one this caller may act on: the workspace models nothing. An empty
    // scan dir states exactly that, and the probes that need a measure report
    // it as nothing-to-check rather than as a failure.
    if views.is_empty() && topics.is_empty() {
        return empty_scan_dir();
    }

    materialise(config_manager.origin(), &views, &topics).await
}

/// Write compiled rows into the tempdir layout airlayer expects, and cache the
/// result under `origin`'s revision. Shared by both resolvers so the two can
/// only differ in WHICH source they accept, never in how a compiled one is laid
/// out or cached.
async fn materialise(
    origin: Origin,
    views: &[CompiledArtifact],
    topics: &[CompiledArtifact],
) -> Result<ScanDir, ArtifactError> {
    let dir = TempDir::new().map_err(io_err)?;
    let scan_path = dir.path().join("semantics");
    let views_dir = scan_path.join("views");
    let topics_dir = scan_path.join("topics");
    tokio::fs::create_dir_all(&views_dir)
        .await
        .map_err(io_err)?;
    tokio::fs::create_dir_all(&topics_dir)
        .await
        .map_err(io_err)?;

    let first_view = write_artifacts(views, &views_dir, "view.yml")
        .await
        .map_err(io_err)?;
    let first_topic = write_artifacts(topics, &topics_dir, "topic.yml")
        .await
        .map_err(io_err)?;

    // info, not debug: this now fires once per revision per process rather than
    // once per request, and it is the line that shows the cache working.
    tracing::info!(
        ?origin,
        views = views.len(),
        topics = topics.len(),
        path = %scan_path.display(),
        "semantic scan: materialised from the compile boundary"
    );

    // What `cached_scan` stats to tell a live tree from a reaped one: a file
    // this materialise wrote, or the root when no row produced one.
    let sentinel = first_view
        .or(first_topic)
        .unwrap_or_else(|| scan_path.clone());
    let scan = ScanDir::Materialised {
        _dir: Arc::new(dir),
        scan_path,
    };
    if let Some(key) = revision_key(origin) {
        remember_scan(key, &scan, sentinel);
    }
    Ok(scan)
}

/// `origin`'s revision from the cache, else `read` — which runs for one
/// request at a time per revision.
///
/// The per-revision lock is what makes a burst cheap. A sheet fires its
/// queries together, so on a cold replica they all miss at once; unguarded,
/// every one of them would read the boundary and materialise the same
/// revision, and all but one copy would be thrown away. Here the first takes
/// the lock and fills the cache, and the rest wait on it and are served from
/// the cache.
///
/// A read that fails — or answers with something the cache does not keep, like
/// an empty revision — caches nothing, and each waiter reads for itself rather
/// than inheriting the error. The lock records that outcome, so those waiters
/// then read side by side: queueing them one at a time would only serialise
/// the same miss, and make a revision that keeps failing slower with every
/// request in the queue.
///
/// `read` is a future that has not been polled: a hit drops it unrun.
async fn scan_once(
    origin: Origin,
    read: impl Future<Output = Result<ScanDir, ArtifactError>>,
) -> Result<ScanDir, ArtifactError> {
    // A revision this process has already materialised is the whole answer: no
    // boundary query, no S3 GET, no disk write.
    if let Some(scan) = cached_scan(origin) {
        return Ok(scan);
    }
    // The working copy is never cached, so there is nothing to wait for.
    let Some(key) = revision_key(origin) else {
        return read.await;
    };
    let flight = in_flight(key);
    let mut cached_nothing = flight.lock().await;
    // Whoever held the lock before this request may have just filled the cache.
    if let Some(scan) = cached_scan(origin) {
        return Ok(scan);
    }
    if *cached_nothing {
        drop(cached_nothing);
        return read.await;
    }
    let scan = read.await;
    *cached_nothing = cached_scan(origin).is_none();
    scan
}

/// The async lock for a revision some request is materialising right now,
/// guarding whether the last read under it cached nothing.
///
/// Held as `Weak`, and dead entries are pruned on every call, so the map never
/// outgrows the misses in flight — and a flag never outlives the burst that set
/// it. A poisoned map degrades to an unshared lock: no single-flight, but no
/// failure either.
fn in_flight(key: (Uuid, Uuid)) -> Arc<tokio::sync::Mutex<bool>> {
    type Flights = Mutex<HashMap<(Uuid, Uuid), Weak<tokio::sync::Mutex<bool>>>>;
    static FLIGHTS: OnceLock<Flights> = OnceLock::new();
    let Ok(mut flights) = FLIGHTS.get_or_init(Flights::default).lock() else {
        return Arc::new(tokio::sync::Mutex::new(false));
    };
    flights.retain(|_, flight| flight.strong_count() > 0);
    if let Some(flight) = flights.get(&key).and_then(Weak::upgrade) {
        return flight;
    }
    let flight = Arc::new(tokio::sync::Mutex::new(false));
    flights.insert(key, Arc::downgrade(&flight));
    flight
}

/// Max `(workspace, revision)` scans a process keeps materialised, across every
/// workspace. [`REVISIONS_PER_WORKSPACE`] bounds each workspace's share.
const SCAN_CACHE_CAP: usize = 256;

/// How many revisions of one workspace the scan cache keeps: the current one,
/// and the one a promote window's stragglers are still pinned to.
const REVISIONS_PER_WORKSPACE: usize = 2;

/// A cached materialisation, and the file that shows its tree is still there.
#[derive(Clone)]
struct CachedScan {
    scan: ScanDir,
    sentinel: PathBuf,
}

/// Process-global LRU of materialised semantic scans, keyed by
/// `(workspace_id, revision_id)`.
///
/// A revision's rows are written in the one transaction that finalises it and
/// never change afterwards (`oxy_compile::writer::finalise_revision`; every
/// revision id is a fresh v4), so a hit is always current. A promote moves
/// `current_revision_id`, the request pins the new id, and the lookup misses —
/// a new revision can never be served an old one's files.
///
/// Bounded per workspace as well as in total, because every entry pins a
/// tempdir — on tmpfs, so in RAM, in plenty of container images — and a
/// workspace that promotes all day must not fill the cache with superseded
/// models. [`materialise_agent_context`] meets the same pressure by popping a
/// workspace's prior revisions on insert, freeing the stale tempdir the moment
/// a promote lands. This cache keeps ONE prior revision instead of none: across
/// a promote window requests pinned to the old and the new revision are both
/// in flight, and evicting either on the other's insert would ping-pong
/// re-materialisations on the request hot path. An evicted tempdir is deleted
/// once the last request holding it drops its clone.
///
/// Only the non-empty compiled arm is stored. The working copy is mutable, and
/// an EMPTY revision's answer depends on whether this node holds a working copy
/// right now (`scan_dir_for_empty_revision`), which can change under it.
fn scan_cache() -> &'static Mutex<LruCache<(Uuid, Uuid), CachedScan>> {
    static CACHE: OnceLock<Mutex<LruCache<(Uuid, Uuid), CachedScan>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(SCAN_CACHE_CAP).expect("SCAN_CACHE_CAP is non-zero"),
        ))
    })
}

/// Cache `scan` under `key`, then trim its workspace to the
/// [`REVISIONS_PER_WORKSPACE`] most recently used revisions.
fn remember_scan(key: (Uuid, Uuid), scan: &ScanDir, sentinel: PathBuf) {
    let Ok(mut cache) = scan_cache().lock() else {
        return;
    };
    cache.put(
        key,
        CachedScan {
            scan: scan.clone(),
            sentinel,
        },
    );
    // `iter` runs most- to least-recently used, so whatever of this workspace
    // comes after its first few entries is what goes.
    let superseded: Vec<(Uuid, Uuid)> = cache
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| k.0 == key.0)
        .skip(REVISIONS_PER_WORKSPACE)
        .collect();
    for k in superseded {
        cache.pop(&k);
    }
}

/// The cache key for a compiled revision; `None` for the working copy.
fn revision_key(origin: Origin) -> Option<(Uuid, Uuid)> {
    match origin {
        Origin::Compiled {
            workspace_id,
            revision_id,
        } => Some((workspace_id, revision_id)),
        Origin::Disk => None,
    }
}

/// This process's materialisation of `origin`'s revision, if it has one. The
/// guard is held for the map op and one stat, never across an await.
fn cached_scan(origin: Origin) -> Option<ScanDir> {
    let key = revision_key(origin)?;
    let mut cache = scan_cache().lock().ok()?;
    let entry = cache.get(&key)?.clone();
    // The one way a cached tempdir differs from a per-request one: it lives
    // long enough for something else to delete it. An age-based tmp reaper
    // removes files first and emptied directories after, so a tree can be
    // gutted with every directory still standing — stat the root and airlayer
    // parses an empty model from it on every hit. So stat a file the
    // materialise wrote: still one syscall. On a miss the caller re-materialises.
    if !entry.sentinel.exists() {
        cache.pop(&key);
        return None;
    }
    Some(entry.scan)
}

/// Where a promoted revision with zero semantic rows scans from.
///
/// Split out so it is reachable without a database: the caller above has
/// already made two boundary queries by the time it gets here, which puts the
/// decision out of reach of a unit test.
fn scan_dir_for_empty_revision<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<ScanDir, ArtifactError> {
    match config_manager.semantics_scan_dir() {
        Ok(root) => Ok(ScanDir::Real(root)),
        // Two ways to have no disk to check the revision against: no slot at
        // all, or a slot naming a directory this node does not hold. Both are
        // nodes where zero semantic rows IS the answer. Named rather than `_`
        // so a future variant cannot silently become an empty scan — which is
        // absence-as-emptiness, one level in.
        Err(ArtifactError::NoSource | ArtifactError::WorkspaceUnavailable(_)) => empty_scan_dir(),
        Err(e) => Err(e),
    }
}

/// An empty scan directory, for a promoted revision that genuinely has no
/// semantic files on a node with no working copy to check it against.
///
/// Laid out like the materialised arm — `semantics/{views,topics}` — so
/// airlayer walks the shape it always walks and finds nothing, rather than
/// failing on a directory that is not there.
fn empty_scan_dir() -> Result<ScanDir, ArtifactError> {
    let dir = TempDir::new().map_err(io_err)?;
    let scan_path = dir.path().join("semantics");
    std::fs::create_dir_all(scan_path.join("views")).map_err(io_err)?;
    std::fs::create_dir_all(scan_path.join("topics")).map_err(io_err)?;
    Ok(ScanDir::Materialised {
        _dir: Arc::new(dir),
        scan_path,
    })
}

fn io_err(e: std::io::Error) -> ArtifactError {
    ArtifactError::Config(OxyError::IOError(e.to_string()))
}

/// Which semantic entity a single-read resolves.
#[derive(Clone, Copy)]
pub enum SemanticEntity {
    View,
    Topic,
}

/// Materialise the promoted semantic scan and return it together with the
/// on-disk path of ONE requested view/topic, so a Path-based parser (airlayer)
/// can parse that file WITH full scan context for cross-entity reference
/// resolution (a topic hydrates its referenced views from the same scan dir).
///
/// Returns `Ok(None)` when the workspace isn't promoted or the requested file
/// isn't a compiled row — the caller then falls through to the FS, exactly like
/// every other reader. The returned [`ScanDir`] guard MUST be held
/// until parsing finishes.
///
/// The scan itself comes from [`scan_dir`]'s per-revision cache, so a detail
/// read no longer re-materialises the whole promoted model (S3 + disk writes)
/// on every open; what it still pays per call is the one boundary query that
/// maps `file_path` to its row.
pub async fn materialise_semantic_entity<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
    entity: SemanticEntity,
    file_path: &str,
) -> Result<Option<(ScanDir, PathBuf)>, ArtifactError> {
    let scan = scan_dir(config_manager).await?;
    // On a working copy the file is already where the caller expects it; the
    // materialised arm has to map the row's workspace-relative path into the
    // tempdir the way `write_artifacts` did.
    let (subdir, suffix) = match entity {
        SemanticEntity::View => ("views", "view.yml"),
        SemanticEntity::Topic => ("topics", "topic.yml"),
    };
    let target = match &scan {
        // `file_path` is base64 out of the request URL. Joining it onto the
        // root is a traversal: `../../etc/x.yml` resolves outside the
        // workspace and gets read and parsed. The read this replaced went
        // through `ConfigManager::resolve_file` -> `fs_link` ->
        // `validate_path_within_project`, which canonicalises both sides and
        // rejects an escape; moving the read here left the guard behind.
        //
        // `fs_link` rather than a hand-rolled `..` check: a symlink inside the
        // workspace can point out of it, and only canonicalising sees that.
        ScanDir::Real(_) => {
            let Some(working_copy) = config_manager.working_copy() else {
                return Ok(None);
            };
            match working_copy.storage().fs_link(file_path).await {
                Ok(resolved) => PathBuf::from(resolved),
                Err(e) => {
                    tracing::warn!(
                        file_path,
                        error = %e,
                        "semantic single-read: path rejected as outside the workspace"
                    );
                    return Ok(None);
                }
            }
        }
        ScanDir::Materialised { scan_path, .. } => {
            let rows = match entity {
                SemanticEntity::View => config_manager.compiled_semantic_views().await?,
                SemanticEntity::Topic => config_manager.compiled_semantic_topics().await?,
            }
            .unwrap_or_default();
            let Some(row) = rows.into_iter().find(|r| r.file_path == file_path) else {
                return Ok(None);
            };
            match derive_target_path(&scan_path.join(subdir), &row.file_path, &row.name, suffix) {
                Some(target) => target,
                // The row was not materialised, so there is nothing to read.
                None => return Ok(None),
            }
        }
    };
    if !target.exists() {
        tracing::warn!(
            file_path,
            "semantic single-read: no file at the resolved path; caller falls through"
        );
        return Ok(None);
    }
    Ok(Some((scan, target)))
}

/// Write each artifact's `definition` JSONB to a YAML file under
/// `target_dir`, recreating the subdirectory layout from the row's
/// `file_path` so two artifacts that share a basename across
/// directories (e.g. `a/orders.view.yml` and `b/orders.view.yml` with
/// distinct in-file `name`s) don't collide.
///
/// The compile-boundary PK on `semantic_views` is `(revision_id,
/// name)`, so distinct names with the same basename DO ship as two
/// rows. A flat tempdir would silently overwrite one with the other
/// and airlayer would see only the survivor at query time — a
/// silent-data-loss bug under the "two folders, same file basename"
/// pattern.
///
/// Returns the first file written, if any row produced one: the scan cache
/// stats it to tell a live tree from a reaped one.
async fn write_artifacts(
    rows: &[CompiledArtifact],
    target_dir: &std::path::Path,
    fallback_suffix: &str,
) -> Result<Option<std::path::PathBuf>, std::io::Error> {
    use futures::stream::{StreamExt, TryStreamExt};
    use std::collections::HashSet;

    // Inside `materialise_semantic_scan` we already build
    // `<tmp>/semantics/views` and `<tmp>/semantics/topics`. The
    // compile rows' `file_path` is workspace-relative (e.g.
    // `semantics/views/orders.view.yml` or
    // `team-a/orders.view.yml`). We strip the leading
    // `semantics/<kind>/` prefix when present so the result lands as
    // a child of `target_dir`; otherwise we append the row's relative
    // path verbatim. The point is: preserve every directory segment
    // BELOW the kind, so distinct rows land at distinct paths.
    let mut written: HashSet<std::path::PathBuf> = HashSet::new();
    // Rows are cloned into their fetches rather than borrowed. A stream of
    // futures each holding a `&CompiledArtifact` trips rustc's higher-ranked
    // auto-trait check in every axum handler that awaits a scan ("implementation
    // of `Send` is not general enough"). The clone is cheap next to the GET it
    // feeds, and it runs once per revision.
    let targets: Vec<(std::path::PathBuf, CompiledArtifact)> = rows
        .iter()
        .filter_map(|row| {
            let target =
                derive_target_path(target_dir, &row.file_path, &row.name, fallback_suffix)?;
            if !written.insert(target.clone()) {
                tracing::warn!(
                    file_path = %row.file_path,
                    target = %target.display(),
                    "semantic scan: two rows materialise to the same tempdir path — second write overwrites the first"
                );
            }
            Some((target, row.clone()))
        })
        .collect();

    // Bodies resolve concurrently — each is an S3 GET when the blob bucket is
    // set, and one after another those GETs were the whole cost of a scan —
    // while the writes still land one at a time, in row order. `buffered`
    // rather than `buffer_unordered` is what keeps that order, and it is
    // load-bearing: two rows that share a path still end last-writer-wins, as
    // the warning above promises, and the first failing row is still the one
    // reported.
    let first = targets.first().map(|(target, _)| target.clone());
    futures::stream::iter(targets.into_iter().map(|(target, row)| async move {
        let body = resolve_body(&row).await.map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("resolve body for semantic row {}: {e}", row.name),
            )
        })?;
        Ok::<_, std::io::Error>((target, body))
    }))
    .buffered(MAX_CONCURRENT_BODY_FETCHES)
    .try_for_each(|(target, body)| async move {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(target, body).await
    })
    .await?;
    Ok(first)
}

/// How many row bodies one materialise resolves at once. Bounded so a large
/// semantic model cannot open unbounded S3 connections; shared by both writers
/// that fetch blobs.
const MAX_CONCURRENT_BODY_FETCHES: usize = 16;

/// Resolve a row's canonical body. Prefer the S3 blob when
/// `compiled_sql_blob_key` is set AND the bucket is configured; on
/// any S3 miss / transport error fall back to serializing the in-row
/// `definition` JSONB. The fall-through is what keeps semantic
/// queries working when the bucket is misconfigured or the blob
/// genuinely disappeared — Postgres always has the canonical body.
async fn resolve_body(row: &CompiledArtifact) -> Result<Vec<u8>, String> {
    if let Some(key) = row.blob_key.as_deref() {
        match oxy_compile::blob_store::get_blob(key).await {
            Ok(Some(bytes)) => return Ok(bytes),
            Ok(None) => {
                // Bucket not configured. Treat as a normal fall-through.
            }
            Err(e) => {
                tracing::warn!(
                    key,
                    error = %e,
                    "semantic scan: blob fetch failed; falling back to in-row definition"
                );
            }
        }
    }
    serde_yaml::to_string(&row.definition)
        .map(|s| s.into_bytes())
        .map_err(|e| format!("yaml: {e}"))
}

/// Strip the `semantics/<kind>/` prefix from a workspace-relative path
/// when present (the airlayer loader already gives us the kind-rooted
/// target_dir). Falls back to a sanitised version of the row name when
/// the path has no `file_name()` (e.g. trailing slash), or when the path
/// would leave `target_dir`.
///
/// `None` for an unsafe path rather than a clamped one: the two writers below
/// already skip-and-warn (`write_context_artifacts`, `write_verified_queries`),
/// and a silently rewritten destination is worse than a dropped row — the
/// operator sees neither.
fn derive_target_path(
    target_dir: &std::path::Path,
    file_path: &str,
    row_name: &str,
    fallback_suffix: &str,
) -> Option<std::path::PathBuf> {
    let trimmed = file_path
        .strip_prefix("semantics/views/")
        .or_else(|| file_path.strip_prefix("semantics/topics/"))
        .unwrap_or(file_path);
    if trimmed.is_empty() {
        return Some(target_dir.join(format!("{}.{}", row_name, fallback_suffix)));
    }
    // Same guard the sibling writers apply. `file_path` is compiler-produced,
    // which is why this looked safe — but the compiler reads it out of a
    // workspace YAML, and both siblings decided not to trust that. One writer
    // trusting it is the gap: this is the one that lands files where an
    // airlayer scan will read them.
    let rel = std::path::Path::new(trimmed);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        tracing::warn!(
            file_path,
            "semantic scan: skipping a row whose file_path leaves the scan dir"
        );
        return None;
    }
    Some(target_dir.join(trimmed))
}

/// A workspace's compiled context entities materialised at their
/// workspace-relative paths so an analytics agent's `context:` globs resolve.
/// The tempdir is `Arc`-held and cached per (workspace, revision): cloning a
/// `MaterialisedContext` shares the same dir, which survives until both the
/// cache entry and every in-flight clone have dropped (Arc refcount).
#[derive(Clone)]
pub struct MaterialisedContext {
    _dir: Arc<TempDir>,
    pub root: PathBuf,
}

/// Max distinct `(workspace, revision)` contexts a process keeps materialised.
/// Each entry pins a tempdir (the full semantic model + automations + verified
/// SQL) on local disk for as long as it's cached, so an UNBOUNDED map would
/// grow one tempdir per distinct workspace the process ever served — monotonic
/// `/tmp` + inode pressure until restart. That bites hardest on a long-lived
/// `oxy worker`, which drains a global, affinity-free queue across every tenant
/// (a serve replica is ring-hash-sharded to a workspace subset). The LRU caps
/// it: the least-recently-served workspace's tempdir drops (Arc refcount →
/// `TempDir` cleanup) once evicted. 256 covers a large hot set while bounding
/// worst-case resident disk to a few hundred small text trees.
const CONTEXT_CACHE_CAP: usize = 256;

/// Process-global, size-bounded LRU of materialised contexts, keyed by
/// `(workspace_id, revision_id)`. Lets the per-request materialise — writing
/// every view / topic / automation / verified-SQL to a fresh `/tmp` dir — run
/// ONCE per promoted revision instead of on every request. Revisions are
/// immutable, so a hit is always current; a promote yields a new `revision_id`
/// → miss → re-materialise (and the prior revision of that workspace is dropped
/// eagerly on insert, so a busy workspace never pins two tempdirs).
///
/// `std::sync::Mutex` (not DashMap): `LruCache::get` mutates recency so it needs
/// `&mut`, and the guard is only ever held for sync map ops — never across an
/// `.await` — so it can't block the runtime.
fn context_cache() -> &'static Mutex<LruCache<(Uuid, Uuid), MaterialisedContext>> {
    static CACHE: OnceLock<Mutex<LruCache<(Uuid, Uuid), MaterialisedContext>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(CONTEXT_CACHE_CAP).expect("CONTEXT_CACHE_CAP is non-zero"),
        ))
    })
}

/// Materialise the workspace's promoted-revision **context** entities into a
/// tempdir at their workspace-relative `file_path`, e.g.
/// `<tmp>/semantics/views/orders.view.yml`. The analytics run resolves an
/// agent's `context:` globs (`./semantics/**/*`) against this root, which lets a
/// stateless serve replica — with no working copy — build the semantic catalog
/// and discover the databases the views reference (`datasource:`), instead of
/// globbing an absent filesystem and silently ending up with "no databases
/// configured".
///
/// Scope: semantic views + topics + automations + verified `.sql` — the complete
/// `context:` set an analytics run globs. The result is cached per
/// (workspace, revision), so the materialise runs once per promoted revision.
///
/// `Ok(None)` when the workspace isn't promoted or has no semantic rows — the
/// caller then falls through to the FS workspace path.
pub async fn materialise_agent_context<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<Option<MaterialisedContext>, ArtifactError> {
    // Cache key: the revision actually being served (immutable). A hit returns
    // the warm tempdir without re-reading the rows or re-writing the dir. The
    // manager's revision IS the one the request was pinned to, so key and
    // content cannot name different revisions.
    let Origin::Compiled {
        workspace_id,
        revision_id,
    } = config_manager.origin()
    else {
        return Ok(None); // reading the working copy → caller uses it directly
    };
    if let Some(hit) = context_cache()
        .lock()
        .ok()
        .and_then(|mut c| c.get(&(workspace_id, revision_id)).cloned())
    {
        return Ok(Some(hit));
    }

    // `?`, not `.unwrap_or_default()`. Each of these used to swallow its error
    // into an empty list, and four empty lists took the `Ok(None)` exit below —
    // which the caller reports as "workspace not promoted, recompile it". A
    // Postgres hiccup therefore told an operator to recompile a workspace that
    // was compiled fine, while the caller's `Err` arm — the one that names this
    // exact failure — could never fire, because the error had already been
    // spent.
    //
    // Automations + verified `.sql` complete the agent's `context:` set: the
    // solver globs them from `base_dir`, and a matched verified query is read
    // back by path at run time (`solver/specifying`). Without these in the
    // tempdir the run still hits the real FS for them — the leak this closes.
    //
    // Concurrent: four independent reads of the same revision, so serialising
    // them only added latency. `try_join!` keeps the `?` semantics above —
    // the first error still wins and still propagates.
    let (views, topics, automations, verified) = tokio::try_join!(
        config_manager.compiled_semantic_views(),
        config_manager.compiled_semantic_topics(),
        config_manager.compiled_automation_artifacts(),
        config_manager.compiled_verified_queries(),
    )?;
    let views = views.unwrap_or_default();
    let topics = topics.unwrap_or_default();
    let automations = automations.unwrap_or_default();
    let verified = verified.unwrap_or_default();
    if views.is_empty() && topics.is_empty() && automations.is_empty() && verified.is_empty() {
        // Genuinely nothing compiled. Distinct from every error above, and the
        // caller acts on the difference: this one says "promote the workspace",
        // an `Err` says "the boundary is unavailable, retry".
        return Ok(None);
    }

    let dir = Arc::new(TempDir::new().map_err(io_err)?);
    let root = dir.path().to_path_buf();
    write_context_artifacts(&views, &root)
        .await
        .map_err(io_err)?;
    write_context_artifacts(&topics, &root)
        .await
        .map_err(io_err)?;
    write_context_artifacts(&automations, &root)
        .await
        .map_err(io_err)?;
    write_verified_queries(&verified, &root)
        .await
        .map_err(io_err)?;

    let ctx = MaterialisedContext {
        _dir: dir,
        root: root.clone(),
    };
    // Insert under the bounded LRU. First drop any PRIOR revision of this
    // workspace so a promote frees the stale tempdir eagerly (Arc refcount →
    // cleanup once in-flight clones release) instead of waiting for LRU
    // eviction; then `put`, which itself evicts the least-recently-served
    // workspace when the cache is at CONTEXT_CACHE_CAP. (Lock held for sync map
    // ops only — never across an await.)
    if let Ok(mut cache) = context_cache().lock() {
        let stale: Vec<(Uuid, Uuid)> = cache
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| k.0 == workspace_id && k.1 != revision_id)
            .collect();
        for k in stale {
            cache.pop(&k);
        }
        cache.put((workspace_id, revision_id), ctx.clone());
    }

    // info (not debug like the per-request boundary reads): the single
    // confirmation that the boundary context path — not an absent FS — served
    // this revision. Logged on the materialise (a miss), not on warm hits.
    tracing::info!(
        workspace_id = %workspace_id,
        revision_id = %revision_id,
        views = views.len(),
        topics = topics.len(),
        automations = automations.len(),
        verified = verified.len(),
        root = %root.display(),
        "context materialise: materialised + cached agent context from compile boundary"
    );
    Ok(Some(ctx))
}

/// Write each artifact's resolved body to `<root>/<file_path>`, recreating the
/// workspace-relative directory layout so the agent's globs match. Paths that
/// are absolute or escape the root (`..`) are skipped defensively — `file_path`
/// is compiler-produced and workspace-relative, but we never trust it into a
/// `join` blindly.
async fn write_context_artifacts(
    rows: &[CompiledArtifact],
    root: &std::path::Path,
) -> Result<(), std::io::Error> {
    use futures::stream::TryStreamExt;

    // Resolve target paths up front: drop unsafe paths (absolute / `..`) and
    // dedup by target. The compile PK is (revision_id, name), so two named
    // entities can share one physical `file_path`; without deduping, the
    // concurrent writes below would race on that path (truncated / interleaved
    // output) instead of the serial version's clean last-writer-wins. Mirrors
    // the sibling `write_artifacts` collision guard.
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<(std::path::PathBuf, &CompiledArtifact)> = rows
        .iter()
        .filter_map(|row| {
            let rel = std::path::Path::new(&row.file_path);
            if rel.is_absolute()
                || rel
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                tracing::warn!(file_path = %row.file_path,
                    "context materialise: skipping unsafe file_path");
                return None;
            }
            let target = root.join(rel);
            if !seen.insert(target.clone()) {
                tracing::warn!(file_path = %row.file_path,
                    "context materialise: two rows materialise to the same path — dropping duplicate");
                return None;
            }
            Some((target, row))
        })
        .collect();

    // Resolve each body (possibly an S3 GET for blob-keyed rows) + write it
    // concurrently, bounded so a large semantic model can't open unbounded S3
    // connections at once. Doing this serially would add every row's blob-fetch
    // latency to the run's startup; in-row JSONB rows resolve without any S3.
    futures::stream::iter(targets.into_iter().map(Ok::<_, std::io::Error>))
        .try_for_each_concurrent(MAX_CONCURRENT_BODY_FETCHES, |(target, row)| async move {
            // create_dir_all is idempotent, so concurrent callers writing into
            // the same `semantics/views` dir don't race destructively.
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let body = resolve_body(row).await.map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("resolve body for context row {}: {e}", row.name),
                )
            })?;
            tokio::fs::write(target, body).await
        })
        .await
}

/// Write each verified query's raw SQL to `<root>/<file_path>`. Unlike the YAML
/// artifacts these carry no `definition` — the body IS the `.sql` text (with its
/// `/* oxy: ... */` header) — so they bypass `resolve_body` and write verbatim.
/// Same path-safety + dedup guard as `write_context_artifacts`.
async fn write_verified_queries(
    rows: &[VerifiedQueryEntry],
    root: &std::path::Path,
) -> Result<(), std::io::Error> {
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let rel = std::path::Path::new(&row.file_path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            tracing::warn!(file_path = %row.file_path,
                "context materialise: skipping unsafe verified-query path");
            continue;
        }
        let target = root.join(rel);
        if !seen.insert(target.clone()) {
            tracing::warn!(file_path = %row.file_path,
                "context materialise: two verified queries materialise to the same path — dropping duplicate");
            continue;
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&target, &row.content).await?;
    }
    Ok(())
}

/// Tempdir + path to a materialised `.monitor.yml`. Drop the wrapper
/// to clean up.
pub struct MaterialisedMonitorConfig {
    _dir: TempDir,
    pub config_path: PathBuf,
}

/// Materialise the workspace's `.monitor.yml` from
/// `monitor_configs.definition` into a tempdir. The metric-monitoring
/// crate (and its scan-workspace entry point) accepts a file path;
/// this hands it a Postgres-sourced file. Returns `Ok(None)` when the
/// workspace isn't promoted or no monitor row exists — caller falls through
/// to the FS path.
pub async fn materialise_monitor_config<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<Option<MaterialisedMonitorConfig>, ArtifactError> {
    // Only the compiled arm needs materialising — on a working copy the file is
    // already at `<root>/.monitor.yml`, which is what the caller falls back to.
    if config_manager.revision_id().is_none() {
        return Ok(None);
    }
    // `?`, not `.unwrap_or(None)`: a boundary that could not be read is not a
    // workspace without monitors. The caller falls back to the FS on `None`,
    // and on a replica that FS is not there.
    let Some(definition) = config_manager.monitor_config().await? else {
        return Ok(None);
    };
    let yaml = serde_yaml::to_string(&definition)
        .map_err(|e| OxyError::SerializerError(format!("re-serialise monitor config: {e}")))?;
    let dir = TempDir::new().map_err(io_err)?;
    let config_path = dir.path().join(".monitor.yml");
    tokio::fs::write(&config_path, yaml).await.map_err(io_err)?;
    tracing::debug!(
        path = %config_path.display(),
        "materialise_monitor_config: materialised from compile boundary"
    );
    Ok(Some(MaterialisedMonitorConfig {
        _dir: dir,
        config_path,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use VerifiedQueryEntry as CompiledVerifiedQuery;

    fn vq(file_path: &str, content: &str) -> CompiledVerifiedQuery {
        CompiledVerifiedQuery {
            file_path: file_path.to_string(),
            content_sha256: String::new(),
            content: content.to_string(),
        }
    }

    /// The combination that used to be a permanent 503: a workspace that is
    /// compiled and promoted, has no `.view.yml`/`.topic.yml` at all — a
    /// legitimate state — and is served by a node that does not hold the files.
    ///
    /// Built the way production builds it: the slot is FULL and the directory
    /// is absent, because `effective_workspace_path` returns the database
    /// column unstat-ed. The first version of this test used
    /// `without_working_copy()` — a slot-less manager — and so passed while the
    /// arm under test was unreachable on every real replica.
    #[tokio::test]
    async fn an_empty_revision_scans_empty_on_a_node_without_the_files() {
        let parent = TempDir::new().expect("tempdir");
        let absent = parent.path().join("never-cloned");
        let diskless = crate::config::ConfigBuilder::new()
            .with_workspace_path(&absent)
            .expect("workspace path")
            .build_with_working_copy(Origin::Disk, crate::config::OnMissing::Empty)
            .await
            .expect("a manager builds from the database column, unstat-ed")
            .into_read_only();

        assert!(
            diskless.working_copy().is_some(),
            "the slot is full on a replica — that is the whole trap"
        );
        assert!(!absent.is_dir(), "and the directory is not there");

        let scan = scan_dir_for_empty_revision(&diskless)
            .expect("zero semantic rows is an answer, not a fault");

        // Airlayer walks the shape it always walks and finds nothing — which
        // on this node is the truth, not a guess about files it cannot see.
        assert!(scan.path().join("views").is_dir());
        assert!(scan.path().join("topics").is_dir());
        assert_eq!(
            std::fs::read_dir(scan.path().join("views"))
                .expect("read views")
                .count(),
            0
        );
    }

    /// The other half, and the reason the empty dir is not simply always
    /// right: a node holding the files may have MORE than the revision does,
    /// and an empty scan dir would hide them.
    #[tokio::test]
    async fn an_empty_revision_still_prefers_a_working_copy_that_exists() {
        let dir = TempDir::new().expect("tempdir");
        let manager = workspace_manager(dir.path()).await;

        let scan = scan_dir_for_empty_revision(&manager).expect("a working copy is a scan dir");

        assert_eq!(
            scan.path().canonicalize().expect("canonicalise scan"),
            dir.path().canonicalize().expect("canonicalise workspace"),
            "the working copy wins when there is one"
        );
    }

    /// A workspace root with a `config.yml` and nothing else — the shape both
    /// tests above start from.
    async fn workspace_manager(
        root: &std::path::Path,
    ) -> ConfigManager<crate::config::WorkingCopy> {
        tokio::fs::write(root.join("config.yml"), "models: []\ndatabases: []\n")
            .await
            .expect("write config");

        crate::config::ConfigBuilder::new()
            .with_workspace_path(root)
            .expect("workspace path")
            .build_with_working_copy(Origin::Disk, crate::config::OnMissing::Empty)
            .await
            .expect("manager")
    }

    /// The half `a_single_read_cannot_escape_the_workspace` cannot cover: that
    /// the guard it added still lets a legitimate read through.
    ///
    /// The guard routes through `fs_link`, which resolves against
    /// `FsStorage::project_path`; the join it replaced used
    /// `WorkingCopy::root`. Those are the same value today — `WorkingCopy::new`
    /// sets `root` FROM `storage.project_path()` — and nothing but this test
    /// says so. Separate them and every view/topic detail read in the IDE
    /// silently answers "not found", with the negative test still green.
    #[tokio::test]
    async fn a_legitimate_single_read_still_resolves() {
        let dir = TempDir::new().expect("tempdir");
        let manager = workspace_manager(dir.path()).await;
        tokio::fs::create_dir_all(dir.path().join("semantics/views"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            dir.path().join("semantics/views/orders.view.yml"),
            "name: orders\n",
        )
        .await
        .expect("write view");

        let (_scan, path) = materialise_semantic_entity(
            &manager,
            SemanticEntity::View,
            "semantics/views/orders.view.yml",
        )
        .await
        .expect("a path inside the workspace is not an error")
        .expect("a path inside the workspace must resolve");

        assert!(
            path.ends_with("semantics/views/orders.view.yml"),
            "resolved to {}",
            path.display()
        );
    }

    /// The traversal `fs_link` used to stop. `file_path` here is base64 out of
    /// the request URL, so `../` in it is caller-controlled — and the working
    /// copy arm used to resolve it through `ConfigManager::resolve_file`, which
    /// canonicalises and rejects an escape. Moving the read into this module
    /// left that behind for one release.
    #[tokio::test]
    async fn a_single_read_cannot_escape_the_workspace() {
        let outside = TempDir::new().expect("tempdir");
        tokio::fs::write(outside.path().join("secret.view.yml"), "name: secret\n")
            .await
            .expect("write outside");

        let workspace = outside.path().join("ws");
        tokio::fs::create_dir_all(&workspace).await.expect("mkdir");
        tokio::fs::write(workspace.join("config.yml"), "models: []\ndatabases: []\n")
            .await
            .expect("write config");

        let manager = crate::config::ConfigBuilder::new()
            .with_workspace_path(&workspace)
            .expect("workspace path")
            .build_with_working_copy(Origin::Disk, crate::config::OnMissing::Empty)
            .await
            .expect("manager");

        let escaped =
            materialise_semantic_entity(&manager, SemanticEntity::View, "../secret.view.yml")
                .await
                .expect("a rejected path is not an error");

        assert!(
            escaped.is_none(),
            "`../secret.view.yml` resolves outside the workspace and must not be served"
        );
    }

    /// The guard both sibling writers already had. `file_path` is
    /// compiler-produced, but the compiler reads it out of a workspace YAML,
    /// and this is the writer whose output an airlayer scan then reads.
    #[test]
    fn a_row_path_cannot_escape_the_scan_dir() {
        let target = std::path::Path::new("/tmp/scan/views");

        assert!(
            derive_target_path(target, "../../etc/evil.view.yml", "evil", "view.yml").is_none(),
            "a `..` path must be skipped, not clamped"
        );
        assert!(
            derive_target_path(target, "/etc/evil.view.yml", "evil", "view.yml").is_none(),
            "an absolute path must be skipped"
        );
        assert_eq!(
            derive_target_path(
                target,
                "semantics/views/a/orders.view.yml",
                "orders",
                "view.yml"
            ),
            Some(target.join("a/orders.view.yml")),
            "a normal row still keeps every directory segment below the kind"
        );
    }

    // The verified-query writer must reproduce the file at its workspace-relative
    // path (so the agent's `context:` globs match) and must NEVER let a
    // compiler-produced path escape the request tempdir.
    #[tokio::test]
    async fn write_verified_queries_writes_relative_and_blocks_escape() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let rows = vec![
            vq("example_sql/answer.sql", "SELECT 1 /* oxy: */"),
            vq("../escape.sql", "evil"),
            vq("/abs/escape.sql", "evil"),
        ];

        write_verified_queries(&rows, &root).await.unwrap();

        // Safe path: written verbatim, nested parent created.
        let body = tokio::fs::read_to_string(root.join("example_sql/answer.sql"))
            .await
            .unwrap();
        assert_eq!(body, "SELECT 1 /* oxy: */");
        // `..` traversal skipped — nothing lands beside the tempdir.
        assert!(!root.parent().unwrap().join("escape.sql").exists());
        // Absolute path skipped — not silently honoured.
        assert!(!std::path::Path::new("/abs/escape.sql").exists());
    }

    // Two rows mapping to the same target dedupe (first wins) instead of racing
    // or erroring — mirrors `write_context_artifacts`.
    #[tokio::test]
    async fn write_verified_queries_dedupes_same_path() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let rows = vec![vq("q.sql", "first"), vq("q.sql", "second")];
        write_verified_queries(&rows, &root).await.unwrap();
        let body = tokio::fs::read_to_string(root.join("q.sql")).await.unwrap();
        assert_eq!(body, "first");
    }

    /// A compiled view/topic row. No blob key, so `resolve_body` serialises
    /// the in-row definition — what it also does when a GET fails — and the
    /// test never reaches S3, whatever this machine's environment says.
    fn row(file_path: &str, name: &str) -> CompiledArtifact {
        CompiledArtifact {
            name: name.to_string(),
            file_path: file_path.to_string(),
            definition: serde_json::json!({ "name": name }),
            blob_key: None,
        }
    }

    fn body_of(name: &str) -> String {
        serde_yaml::to_string(&serde_json::json!({ "name": name })).expect("yaml")
    }

    /// A manager pinned to a fresh compiled revision, built without a
    /// database — the shape a serve replica's request has.
    fn compiled_manager(root: &std::path::Path) -> ConfigManager<crate::config::WorkingCopy> {
        let config: crate::config::model::Config =
            serde_yaml::from_str("models: []\ndatabases: []\n").expect("config");
        crate::config::ConfigBuilder::new()
            .with_workspace_path(root)
            .expect("workspace path")
            .build_with_provided_config_and_working_copy(
                config,
                Origin::Compiled {
                    workspace_id: Uuid::new_v4(),
                    revision_id: Uuid::new_v4(),
                },
            )
            .expect("manager")
    }

    /// The fix itself. Every semantic request used to re-read the rows, fetch
    /// each body from S3 one at a time and write a fresh tempdir, even when
    /// the engine cache already held the answer. Now a revision is
    /// materialised once, and every later scan of it — from either resolver —
    /// is the same directory.
    ///
    /// This manager has no database behind it, so a scan that went back to the
    /// boundary would fail, or at best land in a fresh tempdir. Getting THIS
    /// directory back is only possible from the cache.
    #[tokio::test]
    async fn a_revision_is_materialised_once_and_then_served_from_the_cache() {
        let root = TempDir::new().expect("tempdir");
        let manager = compiled_manager(root.path());

        let first = materialise(
            manager.origin(),
            &[row("semantics/views/orders.view.yml", "orders")],
            &[row("semantics/topics/sales.topic.yml", "sales")],
        )
        .await
        .expect("materialise");

        let served = scan_dir(&manager).await.expect("a cached revision scans");
        assert!(served.is_materialised(), "it is the revision, not the disk");
        assert_eq!(
            served.path(),
            first.path(),
            "the same tempdir, not a second copy"
        );
        assert_eq!(
            std::fs::read_to_string(served.path().join("views/orders.view.yml")).expect("view"),
            body_of("orders")
        );

        let judged = compiled_scan_dir(&manager)
            .await
            .expect("the compiled-only resolver shares the cache");
        assert_eq!(judged.path(), first.path());
    }

    /// Promotion safety. The key is the revision the request is pinned to, so
    /// a new `current_revision_id` misses and goes back to the boundary; it can
    /// never be handed the previous revision's files.
    #[tokio::test]
    async fn a_new_revision_is_never_served_the_previous_ones_scan() {
        let workspace_id = Uuid::new_v4();
        let old = Origin::Compiled {
            workspace_id,
            revision_id: Uuid::new_v4(),
        };
        let promoted = Origin::Compiled {
            workspace_id,
            revision_id: Uuid::new_v4(),
        };
        let _held = materialise(old, &[row("semantics/views/a.view.yml", "a")], &[])
            .await
            .expect("materialise");

        assert!(cached_scan(old).is_some(), "the old revision is cached");
        assert!(
            cached_scan(promoted).is_none(),
            "a promote must miss, not serve the old files"
        );
        assert!(
            cached_scan(Origin::Disk).is_none(),
            "the working copy is mutable and never cached"
        );
    }

    /// A cached tempdir outlives requests, so something else can delete it
    /// under a long-idle process. A hit must never hand out a path that is
    /// gone; it is a miss, and the caller re-materialises.
    #[tokio::test]
    async fn a_cached_scan_whose_directory_was_reaped_is_a_miss() {
        let origin = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let scan = materialise(origin, &[row("semantics/views/a.view.yml", "a")], &[])
            .await
            .expect("materialise");
        std::fs::remove_dir_all(scan.path()).expect("reap the tempdir");

        assert!(
            cached_scan(origin).is_none(),
            "a reaped directory is a miss"
        );

        // The state an age-based reaper leaves mid-pass: files deleted by age,
        // emptied directories not yet. Every directory still stands, so a stat
        // of the root would call this tree live and airlayer would parse an
        // empty model from it.
        let origin = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let scan = materialise(
            origin,
            &[row("semantics/views/a.view.yml", "a")],
            &[row("semantics/topics/t.topic.yml", "t")],
        )
        .await
        .expect("materialise");
        std::fs::remove_file(scan.path().join("views/a.view.yml")).expect("reap the view");
        std::fs::remove_file(scan.path().join("topics/t.topic.yml")).expect("reap the topic");
        assert!(scan.path().join("views").is_dir(), "the directories stand");

        assert!(cached_scan(origin).is_none(), "a gutted tree is a miss too");
    }

    /// Bounded per workspace, not only in total. A workspace promoting all
    /// day would otherwise fill the cache with superseded models, each pinning
    /// a tempdir. Two stay — the promote window has requests pinned to the old
    /// and the new revision at once — and the rest go.
    #[tokio::test]
    async fn a_workspace_keeps_only_its_two_most_recent_revisions() {
        let neighbour = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let _neighbour = materialise(neighbour, &[row("semantics/views/n.view.yml", "n")], &[])
            .await
            .expect("materialise");

        let workspace_id = Uuid::new_v4();
        let revisions: Vec<Origin> = (0..3)
            .map(|_| Origin::Compiled {
                workspace_id,
                revision_id: Uuid::new_v4(),
            })
            .collect();
        for origin in &revisions {
            materialise(*origin, &[row("semantics/views/a.view.yml", "a")], &[])
                .await
                .expect("materialise");
        }

        assert!(
            cached_scan(revisions[0]).is_none(),
            "the oldest revision is evicted"
        );
        assert!(
            cached_scan(revisions[1]).is_some(),
            "the previous revision stays for the promote window"
        );
        assert!(cached_scan(revisions[2]).is_some(), "the newest stays");
        assert!(
            cached_scan(neighbour).is_some(),
            "another workspace is not trimmed by this one's promotes"
        );
    }

    /// The single-flight. A sheet fires its queries together, so on a cold
    /// replica they all miss at once; each used to read the boundary and
    /// materialise the same revision. Now one does, and the rest are served
    /// the directory it made.
    #[tokio::test]
    async fn a_burst_of_cold_requests_reads_the_boundary_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let origin = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let reads = &AtomicUsize::new(0);
        let read = move || async move {
            reads.fetch_add(1, Ordering::SeqCst);
            // Hold the read open long enough for the whole burst to miss.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            materialise(origin, &[row("semantics/views/a.view.yml", "a")], &[]).await
        };

        let burst = futures::future::join_all((0..7).map(|_| scan_once(origin, read()))).await;

        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "one boundary read for the burst"
        );
        let first = burst[0].as_ref().expect("scan").path().to_path_buf();
        for scan in &burst {
            assert_eq!(
                scan.as_ref().expect("scan").path(),
                first,
                "every request is served the one directory"
            );
        }
    }

    /// The other half of the single-flight. A read that caches nothing — an
    /// error, an empty revision — must not leave the rest of the burst queued
    /// one behind another. Each still reads for itself, since an error is not
    /// shared, but side by side.
    #[tokio::test]
    async fn a_burst_whose_read_caches_nothing_does_not_queue() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let origin = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let reads = &AtomicUsize::new(0);
        let running = &AtomicUsize::new(0);
        let most_running = &AtomicUsize::new(0);
        let read = move || async move {
            reads.fetch_add(1, Ordering::SeqCst);
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most_running.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            running.fetch_sub(1, Ordering::SeqCst);
            Err::<ScanDir, _>(ArtifactError::NoSource)
        };

        let burst = futures::future::join_all((0..7).map(|_| scan_once(origin, read()))).await;

        assert!(burst.iter().all(Result::is_err), "nothing was cached");
        assert_eq!(
            reads.load(Ordering::SeqCst),
            7,
            "each request reads for itself"
        );
        assert!(
            most_running.load(Ordering::SeqCst) > 1,
            "after the first read failed, the rest ran side by side"
        );
    }

    /// A materialised scan is shared, so its tempdir must survive whichever
    /// holder drops first — the cache evicting it mid-request must not delete
    /// files a parse is reading — and must still go when the last one does.
    /// An empty scan dir is never cached, so this test holds every clone.
    #[test]
    fn a_shared_scan_dir_is_deleted_by_its_last_holder() {
        let scan = empty_scan_dir().expect("empty scan dir");
        let root = scan.path().to_path_buf();
        let held = scan.clone();

        drop(scan);
        assert!(root.join("views").is_dir(), "a remaining holder keeps it");

        drop(held);
        assert!(!root.exists(), "the last holder deletes it");
    }

    /// Bodies now resolve concurrently, which is only right if the tree is the
    /// one the serial loop wrote: every row, every directory segment below the
    /// kind, and two rows sharing a path still last-writer-wins.
    #[tokio::test]
    async fn a_concurrent_materialise_writes_what_the_serial_one_did() {
        // Several times the concurrency bound, so the stream runs full windows
        // back to back rather than finishing inside the first one.
        let count = MAX_CONCURRENT_BODY_FETCHES * 3;
        let mut rows: Vec<CompiledArtifact> = (0..count)
            .map(|i| {
                row(
                    &format!("semantics/views/team-{}/v{i}.view.yml", i % 4),
                    &format!("v{i}"),
                )
            })
            .collect();
        rows.push(row("semantics/views/shared.view.yml", "first"));
        rows.push(row("semantics/views/shared.view.yml", "second"));

        let dir = TempDir::new().expect("tempdir");
        write_artifacts(&rows, dir.path(), "view.yml")
            .await
            .expect("write");

        for i in 0..count {
            let path = dir.path().join(format!("team-{}/v{i}.view.yml", i % 4));
            assert_eq!(
                std::fs::read_to_string(&path).expect("every row is written"),
                body_of(&format!("v{i}")),
                "{}",
                path.display()
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("shared.view.yml")).expect("shared"),
            body_of("second"),
            "row order is kept, so the later row still wins the shared path"
        );
    }
}
