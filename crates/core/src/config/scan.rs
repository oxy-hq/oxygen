//! The semantic model a request reads, from the working copy or from a compiled
//! revision, and the in-memory copy of a revision that serves it.
//!
//! The compile boundary stores a workspace's `.view.yml` and `.topic.yml` files
//! as rows. Callers that build an airlayer model out of them used to be handed a
//! directory: the rows were written into a temporary directory laid out the way
//! airlayer's loader expects, and the loader walked it. That was a disk write
//! per revision per process on a pod that is meant to need no disk, and a path
//! that had to be kept alive for as long as anything held it.
//!
//! A compiled revision is now held as the YAML text of its files
//! ([`CompiledSemanticFiles`]) and parsed straight from memory, by the same
//! parser the directory loader uses (`oxy_airlayer_compat::build_layer_from_yaml`),
//! so the two sources differ in nothing but where the text came from. The
//! working copy is still a directory, because it is one.
//!
//! The text is cached per `(workspace_id, revision_id)` (`scan_cache`). The read
//! in front of it was always the expensive part: two boundary queries, then one
//! S3 GET per row whenever `OXY_COMPILE_BLOB_S3_BUCKET` is set (every serve
//! replica sets it), issued one after another, on every semantic request, before
//! the engine cache was even consulted. A revision never changes once written,
//! so the cache is always current, and the one read a revision still pays
//! resolves its bodies concurrently.
//!
//! This module also holds the two readers that still write a directory, because
//! what they hand to their caller is one: [`materialise_agent_context`] (an
//! agent's `context:` globs match files) and [`materialise_monitor_config`].

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use lru::LruCache;
use oxy_shared::errors::OxyError;
use tempfile::TempDir;
use uuid::Uuid;

use super::artifacts::{ArtifactError, CompiledArtifact, VerifiedQueryEntry};
use super::manager::{ConfigManager, DiskSlot, Origin};
use super::storage::ConfigStorage;

/// One compiled `.view.yml` or `.topic.yml`: the row's workspace-relative path,
/// and the YAML text the airlayer loader would have read from that file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticFile {
    pub file_path: String,
    pub yaml: String,
}

/// A compiled revision's semantic files, in the order the directory loader
/// reads them (sorted by path below `semantics/views/` and `semantics/topics/`),
/// so an engine built from either source is built in the same order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CompiledSemanticFiles {
    views: Vec<SemanticFile>,
    topics: Vec<SemanticFile>,
}

impl CompiledSemanticFiles {
    pub fn views(&self) -> &[SemanticFile] {
        &self.views
    }

    pub fn topics(&self) -> &[SemanticFile] {
        &self.topics
    }

    /// The file a compiled row came from, by its workspace-relative path.
    fn file(&self, entity: SemanticEntity, file_path: &str) -> Option<&SemanticFile> {
        let files = match entity {
            SemanticEntity::View => &self.views,
            SemanticEntity::Topic => &self.topics,
        };
        files.iter().find(|f| f.file_path == file_path)
    }
}

/// Where a request reads the semantic model from, whichever source the manager
/// reads.
///
/// On `Origin::Disk` this is the working copy's directory. On
/// `Origin::Compiled` it is the revision's files, in memory: nothing is written
/// anywhere, and a clone shares the one copy.
#[derive(Debug, Clone)]
pub enum ScanDir {
    Real(PathBuf),
    Compiled(Arc<CompiledSemanticFiles>),
}

impl ScanDir {
    /// The directory this scan reads, when it is the working copy. A compiled
    /// revision has none.
    pub fn directory(&self) -> Option<&Path> {
        match self {
            ScanDir::Real(p) => Some(p),
            ScanDir::Compiled(_) => None,
        }
    }

    /// Whether this scan read the compiled revision rather than the working
    /// copy.
    ///
    /// A node can hold a working copy AND be pinned to a revision, so
    /// `ConfigManager::revision_id()` cannot answer this — it reports the pin,
    /// not the source. Anything caching by layer identity has to ask here.
    pub fn is_compiled(&self) -> bool {
        matches!(self, ScanDir::Compiled(_))
    }

    /// Parse the airlayer semantic model. Blocking: it parses every file, so
    /// call it from a blocking task.
    pub fn load_layer(
        &self,
    ) -> Result<oxy_airlayer_compat::SemanticLayer, oxy_airlayer_compat::SemanticError> {
        match self {
            ScanDir::Real(root) => oxy_airlayer_compat::load_layer_from_dir(root),
            ScanDir::Compiled(files) => {
                let views: Vec<&str> = files.views.iter().map(|f| f.yaml.as_str()).collect();
                let topics: Vec<&str> = files.topics.iter().map(|f| f.yaml.as_str()).collect();
                oxy_airlayer_compat::build_layer_from_yaml(&views, &topics)
            }
        }
    }

    /// Parse the semantic model with the `oxy_semantic` parser, which also
    /// validates it. Blocking, like [`Self::load_layer`].
    pub fn parse_semantic_model(
        &self,
    ) -> Result<oxy_semantic::ParseResult, oxy_semantic::SemanticLayerError> {
        match self {
            ScanDir::Real(root) => oxy_semantic::parse_semantic_layer_from_dir(root),
            ScanDir::Compiled(files) => {
                let named = |files: &[SemanticFile]| -> Vec<(PathBuf, String)> {
                    files
                        .iter()
                        .map(|f| (PathBuf::from(&f.file_path), f.yaml.clone()))
                        .collect()
                };
                oxy_semantic::SemanticLayerParser::new(oxy_semantic::ParserConfig::new("semantics"))
                    .parse_sources(&named(&files.views), &named(&files.topics))
            }
        }
    }
}

impl oxy_airlayer_compat::LoadLayer for ScanDir {
    fn load_layer(
        &self,
    ) -> Result<oxy_airlayer_compat::SemanticLayer, oxy_airlayer_compat::SemanticError> {
        ScanDir::load_layer(self)
    }
}

/// The scan for whichever source this manager reads.
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
        // older than the files on disk, and an empty scan would hide them
        // — airlayer reads an empty directory as "scanned, found none", which
        // is absent-reported-as-empty in the other direction.
        //
        // On a node that has none, that reasoning inverts. Zero semantic rows
        // in the promoted revision IS the whole answer, so an empty scan states
        // it rather than overstating it. Erroring here instead made a
        // legitimate workspace — compiled, promoted, no semantic model yet — a
        // permanently retryable 503 on every replica: neither input changes on
        // a retry, so the caller polls forever.
        return scan_dir_for_empty_revision(config_manager);
    }

    load_compiled_scan(config_manager.origin(), &views, &topics).await
}

/// The scan for the COMPILED revision and nothing else.
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
    // The cache only ever holds a compiled revision's files, which is exactly
    // what this resolver would build — so a hit is safe here too.
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
    // scan states exactly that, and the probes that need a measure report it as
    // nothing-to-check rather than as a failure.
    if views.is_empty() && topics.is_empty() {
        return Ok(empty_scan());
    }

    load_compiled_scan(config_manager.origin(), &views, &topics).await
}

/// Resolve compiled rows into a revision's files, and cache the result under
/// `origin`'s revision. Shared by both resolvers so the two can only differ in
/// WHICH source they accept, never in how a compiled one is read or cached.
async fn load_compiled_scan(
    origin: Origin,
    views: &[CompiledArtifact],
    topics: &[CompiledArtifact],
) -> Result<ScanDir, ArtifactError> {
    let files = CompiledSemanticFiles {
        views: resolve_files(views, "view.yml").await.map_err(io_err)?,
        topics: resolve_files(topics, "topic.yml").await.map_err(io_err)?,
    };

    // info, not debug: this fires once per revision per process rather than
    // once per request, and it is the line that shows the cache working.
    tracing::info!(
        ?origin,
        views = files.views.len(),
        topics = files.topics.len(),
        "semantic scan: read from the compile boundary"
    );

    let scan = ScanDir::Compiled(Arc::new(files));
    if let Some(key) = revision_key(origin) {
        remember_scan(key, &scan);
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

/// Max `(workspace, revision)` scans a process keeps, across every workspace.
/// [`REVISIONS_PER_WORKSPACE`] bounds each workspace's share.
const SCAN_CACHE_CAP: usize = 256;

/// How many revisions of one workspace the scan cache keeps: the current one,
/// and the one a promote window's stragglers are still pinned to.
const REVISIONS_PER_WORKSPACE: usize = 2;

/// Process-global LRU of compiled semantic scans, keyed by
/// `(workspace_id, revision_id)`.
///
/// A revision's rows are written in the one transaction that finalises it and
/// never change afterwards (`oxy_compile::writer::finalise_revision`; every
/// revision id is a fresh v4), so a hit is always current. A promote moves
/// `current_revision_id`, the request pins the new id, and the lookup misses —
/// a new revision can never be served an old one's files.
///
/// Bounded per workspace as well as in total, because every entry holds a whole
/// semantic model in memory, and a workspace that promotes all day must not
/// fill the cache with superseded ones. [`materialise_agent_context`] meets the
/// same pressure by popping a workspace's prior revisions on insert. This cache
/// keeps ONE prior revision instead of none: across a promote window requests
/// pinned to the old and the new revision are both in flight, and evicting
/// either on the other's insert would ping-pong re-reads on the request hot
/// path. An evicted scan is freed once the last request holding it drops its
/// clone.
///
/// Only the non-empty compiled arm is stored. The working copy is mutable, and
/// an EMPTY revision's answer depends on whether this node holds a working copy
/// right now (`scan_dir_for_empty_revision`), which can change under it.
fn scan_cache() -> &'static Mutex<LruCache<(Uuid, Uuid), ScanDir>> {
    static CACHE: OnceLock<Mutex<LruCache<(Uuid, Uuid), ScanDir>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(SCAN_CACHE_CAP).expect("SCAN_CACHE_CAP is non-zero"),
        ))
    })
}

/// Cache `scan` under `key`, then trim its workspace to the
/// [`REVISIONS_PER_WORKSPACE`] most recently used revisions.
fn remember_scan(key: (Uuid, Uuid), scan: &ScanDir) {
    let Ok(mut cache) = scan_cache().lock() else {
        return;
    };
    cache.put(key, scan.clone());
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

/// This process's scan of `origin`'s revision, if it has one. The guard is held
/// for the map op, never across an await.
fn cached_scan(origin: Origin) -> Option<ScanDir> {
    let key = revision_key(origin)?;
    let mut cache = scan_cache().lock().ok()?;
    cache.get(&key).cloned()
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
        Err(ArtifactError::NoSource | ArtifactError::WorkspaceUnavailable(_)) => Ok(empty_scan()),
        Err(e) => Err(e),
    }
}

/// A scan with no files, for a promoted revision that genuinely has no semantic
/// files on a node with no working copy to check it against. Loading it yields
/// a model with no views, which is what airlayer reads an empty directory as.
fn empty_scan() -> ScanDir {
    ScanDir::Compiled(Arc::new(CompiledSemanticFiles::default()))
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

/// One requested view or topic, in whichever form its source has it.
#[derive(Debug, Clone)]
pub enum SemanticTarget {
    /// The file in the working copy.
    File(PathBuf),
    /// The compiled revision's copy of the file, which has no path.
    Yaml { file_path: String, yaml: String },
}

/// The scan the manager reads, together with ONE requested view or topic of it,
/// so a parser can read that entity WITH full scan context for cross-entity
/// reference resolution (a topic hydrates its referenced views from the same
/// scan).
///
/// Returns `Ok(None)` when the workspace isn't promoted or the requested file
/// isn't a compiled row — the caller then falls through to the FS, exactly like
/// every other reader.
///
/// The scan itself comes from [`scan_dir`]'s per-revision cache, so a detail
/// read no longer re-reads the whole promoted model on every open; what it
/// still pays per call is nothing but a lookup of `file_path` in it.
pub async fn read_semantic_entity<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
    entity: SemanticEntity,
    file_path: &str,
) -> Result<Option<(ScanDir, SemanticTarget)>, ArtifactError> {
    let scan = scan_dir(config_manager).await?;
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
                Ok(resolved) => {
                    let resolved = PathBuf::from(resolved);
                    if !resolved.exists() {
                        tracing::warn!(
                            file_path,
                            "semantic single-read: no file at the resolved path; caller falls through"
                        );
                        return Ok(None);
                    }
                    SemanticTarget::File(resolved)
                }
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
        ScanDir::Compiled(files) => match files.file(entity, file_path) {
            Some(file) => SemanticTarget::Yaml {
                file_path: file.file_path.clone(),
                yaml: file.yaml.clone(),
            },
            None => return Ok(None),
        },
    };
    Ok(Some((scan, target)))
}

/// Resolve each row's body into a [`SemanticFile`], sorted by the row's path
/// below its kind (`semantics/views/` or `semantics/topics/`) — the order the
/// directory loader reads the same files in.
///
/// Keyed by that relative path, so two rows that share a basename across
/// directories (`a/orders.view.yml` and `b/orders.view.yml` with distinct
/// in-file `name`s) stay two files. The compile-boundary PK on `semantic_views`
/// is `(revision_id, name)`, so distinct names with the same basename DO ship as
/// two rows; keying on the basename would silently keep one of them and airlayer
/// would see only the survivor at query time.
async fn resolve_files(
    rows: &[CompiledArtifact],
    fallback_suffix: &str,
) -> Result<Vec<SemanticFile>, std::io::Error> {
    use futures::stream::{StreamExt, TryStreamExt};

    // Rows are cloned into their fetches rather than borrowed. A stream of
    // futures each holding a `&CompiledArtifact` trips rustc's higher-ranked
    // auto-trait check in every axum handler that awaits a scan ("implementation
    // of `Send` is not general enough"). The clone is cheap next to the GET it
    // feeds, and it runs once per revision.
    let mut seen: BTreeMap<PathBuf, ()> = BTreeMap::new();
    let targets: Vec<(PathBuf, CompiledArtifact)> = rows
        .iter()
        .filter_map(|row| {
            let relative = relative_scan_path(&row.file_path, &row.name, fallback_suffix)?;
            if seen.insert(relative.clone(), ()).is_some() {
                tracing::warn!(
                    file_path = %row.file_path,
                    "semantic scan: two rows read as the same file — the second replaces the first"
                );
            }
            Some((relative, row.clone()))
        })
        .collect();

    // Bodies resolve concurrently — each is an S3 GET when the blob bucket is
    // set, and one after another those GETs were the whole cost of a scan —
    // while the results are still taken in row order. `buffered` rather than
    // `buffer_unordered` is what keeps that order, and it is load-bearing: two
    // rows that share a path still end last-writer-wins, as the warning above
    // promises, and the first failing row is still the one reported.
    let bodies: Vec<(PathBuf, SemanticFile)> =
        futures::stream::iter(targets.into_iter().map(|(relative, row)| async move {
            let body = resolve_body(&row).await.map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("resolve body for semantic row {}: {e}", row.name),
                )
            })?;
            let yaml = String::from_utf8(body).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("semantic row {} is not UTF-8: {e}", row.name),
                )
            })?;
            Ok::<_, std::io::Error>((
                relative,
                SemanticFile {
                    file_path: row.file_path,
                    yaml,
                },
            ))
        }))
        .buffered(MAX_CONCURRENT_BODY_FETCHES)
        .try_collect()
        .await?;

    // Later rows win a shared path; the map then yields them sorted by path.
    let files: BTreeMap<PathBuf, SemanticFile> = bodies.into_iter().collect();
    Ok(files.into_values().collect())
}

/// How many row bodies one read of a revision resolves at once. Bounded so a large
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

/// A compiled row's path below its kind: the workspace-relative path with a
/// leading `semantics/views/` or `semantics/topics/` removed. Falls back to the
/// row name plus `fallback_suffix` when nothing is left (a trailing slash).
///
/// `None` for an unsafe path (absolute, or leaving the scan through `..`)
/// rather than a clamped one: the two writers below already skip-and-warn
/// (`write_context_artifacts`, `write_verified_queries`), and a silently
/// rewritten destination is worse than a dropped row — the operator sees
/// neither. Nothing is written for these rows any more, but the compiler reads
/// the path out of a workspace YAML, and a row the directory loader would not
/// have read must not become part of the model here either.
fn relative_scan_path(file_path: &str, row_name: &str, fallback_suffix: &str) -> Option<PathBuf> {
    let trimmed = file_path
        .strip_prefix("semantics/views/")
        .or_else(|| file_path.strip_prefix("semantics/topics/"))
        .unwrap_or(file_path);
    if trimmed.is_empty() {
        return Some(PathBuf::from(format!("{row_name}.{fallback_suffix}")));
    }
    let rel = Path::new(trimmed);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        tracing::warn!(
            file_path,
            "semantic scan: skipping a row whose file_path leaves the scan"
        );
        return None;
    }
    Some(rel.to_path_buf())
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
/// Scope: semantic views + topics + automations + verified `.sql` — everything
/// in a `context:` set that an analytics run reads by PATH. Markdown documents
/// are deliberately not here: the run takes those by value from
/// [`ConfigManager::context_documents`], on every role, so they need no
/// directory. The result is cached per (workspace, revision), so the
/// materialise runs once per promoted revision.
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

        // An empty model, held in memory — which on this node is the truth, not
        // a guess about files it cannot see.
        assert!(scan.is_compiled(), "nothing on disk is read");
        let layer = scan.load_layer().expect("an empty scan loads");
        assert!(layer.views.is_empty());
        assert!(layer.topics.is_none());
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
            scan.directory()
                .expect("the working copy is a directory")
                .canonicalize()
                .expect("canonicalise scan"),
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

        let (_scan, target) = read_semantic_entity(
            &manager,
            SemanticEntity::View,
            "semantics/views/orders.view.yml",
        )
        .await
        .expect("a path inside the workspace is not an error")
        .expect("a path inside the workspace must resolve");

        let SemanticTarget::File(path) = target else {
            panic!("the working copy answers with a file, got {target:?}");
        };
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

        let escaped = read_semantic_entity(&manager, SemanticEntity::View, "../secret.view.yml")
            .await
            .expect("a rejected path is not an error");

        assert!(
            escaped.is_none(),
            "`../secret.view.yml` resolves outside the workspace and must not be served"
        );
    }

    /// The guard both sibling writers already had. `file_path` is
    /// compiler-produced, but the compiler reads it out of a workspace YAML, and
    /// a row the directory loader would not have read must not become part of
    /// the model here either.
    #[test]
    fn a_row_path_cannot_escape_the_scan() {
        assert!(
            relative_scan_path("../../etc/evil.view.yml", "evil", "view.yml").is_none(),
            "a `..` path must be skipped, not clamped"
        );
        assert!(
            relative_scan_path("/etc/evil.view.yml", "evil", "view.yml").is_none(),
            "an absolute path must be skipped"
        );
        assert_eq!(
            relative_scan_path("semantics/views/a/orders.view.yml", "orders", "view.yml"),
            Some(PathBuf::from("a/orders.view.yml")),
            "a normal row still keeps every directory segment below the kind"
        );
        assert_eq!(
            relative_scan_path("semantics/views/", "orders", "view.yml"),
            Some(PathBuf::from("orders.view.yml")),
            "nothing below the kind falls back to the row's name"
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
    /// each body from S3 one at a time and write a fresh tempdir, even when the
    /// engine cache already held the answer. Now a revision is read once, and
    /// every later scan of it — from either resolver — is the same files.
    ///
    /// This manager has no database behind it, so a scan that went back to the
    /// boundary would fail. Getting these files back is only possible from the
    /// cache.
    #[tokio::test]
    async fn a_revision_is_read_once_and_then_served_from_the_cache() {
        let root = TempDir::new().expect("tempdir");
        let manager = compiled_manager(root.path());

        let first = load_compiled_scan(
            manager.origin(),
            &[row("semantics/views/orders.view.yml", "orders")],
            &[row("semantics/topics/sales.topic.yml", "sales")],
        )
        .await
        .expect("read");

        let served = scan_dir(&manager).await.expect("a cached revision scans");
        assert!(served.is_compiled(), "it is the revision, not the disk");
        let (ScanDir::Compiled(served_files), ScanDir::Compiled(first_files)) = (&served, &first)
        else {
            panic!("both are the revision");
        };
        assert!(
            Arc::ptr_eq(served_files, first_files),
            "the same files, not a second copy"
        );
        assert_eq!(served_files.views()[0].yaml, body_of("orders"));
        assert_eq!(served_files.topics()[0].yaml, body_of("sales"));

        let judged = compiled_scan_dir(&manager)
            .await
            .expect("the compiled-only resolver shares the cache");
        let ScanDir::Compiled(judged_files) = &judged else {
            panic!("the revision");
        };
        assert!(Arc::ptr_eq(judged_files, first_files));
    }

    /// Nothing is written: a compiled scan has no directory at all, so there is
    /// nothing for a tmp reaper to delete and nothing a caller can be handed
    /// that is gone.
    #[tokio::test]
    async fn a_compiled_scan_has_no_directory() {
        let origin = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let scan = load_compiled_scan(origin, &[row("semantics/views/a.view.yml", "a")], &[])
            .await
            .expect("read");
        assert!(scan.directory().is_none());
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
        let _held = load_compiled_scan(old, &[row("semantics/views/a.view.yml", "a")], &[])
            .await
            .expect("read");

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

    /// Bounded per workspace, not only in total. A workspace promoting all day
    /// would otherwise fill the cache with superseded models. Two stay — the
    /// promote window has requests pinned to the old and the new revision at
    /// once — and the rest go.
    #[tokio::test]
    async fn a_workspace_keeps_only_its_two_most_recent_revisions() {
        let neighbour = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let _neighbour =
            load_compiled_scan(neighbour, &[row("semantics/views/n.view.yml", "n")], &[])
                .await
                .expect("read");

        let workspace_id = Uuid::new_v4();
        let revisions: Vec<Origin> = (0..3)
            .map(|_| Origin::Compiled {
                workspace_id,
                revision_id: Uuid::new_v4(),
            })
            .collect();
        for origin in &revisions {
            load_compiled_scan(*origin, &[row("semantics/views/a.view.yml", "a")], &[])
                .await
                .expect("read");
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
    /// replica they all miss at once; each used to read the boundary and read
    /// the same revision. Now one does, and the rest are served what it read.
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
            load_compiled_scan(origin, &[row("semantics/views/a.view.yml", "a")], &[]).await
        };

        let burst = futures::future::join_all((0..7).map(|_| scan_once(origin, read()))).await;

        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "one boundary read for the burst"
        );
        let ScanDir::Compiled(first) = burst[0].as_ref().expect("scan") else {
            panic!("the revision");
        };
        for scan in &burst {
            let ScanDir::Compiled(files) = scan.as_ref().expect("scan") else {
                panic!("the revision");
            };
            assert!(
                Arc::ptr_eq(files, first),
                "every request is served the one copy"
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

    /// The files of a compiled scan load into the model the directory loader
    /// would have built from the same documents, with the entity lookup keyed
    /// by the row's workspace-relative path. The bodies here are the in-row
    /// `definition` (no blob key), so no S3 is reached.
    #[tokio::test]
    async fn a_compiled_scan_loads_the_same_model_as_the_files_would() {
        fn view_row(path: &str, name: &str) -> CompiledArtifact {
            CompiledArtifact {
                name: name.to_string(),
                file_path: path.to_string(),
                definition: serde_json::json!({
                    "name": name,
                    "table": name,
                    "entities": [],
                    "dimensions": [{ "name": "id", "type": "number", "expr": "id" }],
                }),
                blob_key: None,
            }
        }
        let origin = Origin::Compiled {
            workspace_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
        };
        let scan = load_compiled_scan(
            origin,
            &[
                view_row("semantics/views/b/zeta.view.yml", "zeta"),
                view_row("semantics/views/alpha.view.yml", "alpha"),
            ],
            &[],
        )
        .await
        .expect("read");

        let layer = scan.load_layer().expect("the model loads from memory");
        let names: Vec<&str> = layer.views.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            ["alpha", "zeta"],
            "sorted by path below the kind, as the directory loader orders them"
        );
        let parsed = scan
            .parse_semantic_model()
            .expect("and parses and validates");
        assert_eq!(parsed.semantic_layer.views.len(), 2);

        // The entity lookup goes by the row's own path, not the path below the kind.
        let ScanDir::Compiled(files) = &scan else {
            panic!("the revision");
        };
        assert!(
            files
                .file(SemanticEntity::View, "semantics/views/b/zeta.view.yml")
                .is_some()
        );
        assert!(
            files
                .file(SemanticEntity::View, "b/zeta.view.yml")
                .is_none()
        );
        assert!(
            files
                .file(SemanticEntity::Topic, "semantics/views/alpha.view.yml")
                .is_none()
        );
    }

    /// Bodies resolve concurrently, which is only right if the result is the
    /// one the serial loop produced: every row, and two rows sharing a path
    /// still last-writer-wins.
    #[tokio::test]
    async fn a_concurrent_read_keeps_every_row_and_the_last_of_a_shared_path() {
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

        let files = resolve_files(&rows, "view.yml").await.expect("resolve");

        assert_eq!(files.len(), count + 1, "one file per distinct path");
        for i in 0..count {
            let path = format!("semantics/views/team-{}/v{i}.view.yml", i % 4);
            let file = files
                .iter()
                .find(|f| f.file_path == path)
                .unwrap_or_else(|| panic!("{path} is read"));
            assert_eq!(file.yaml, body_of(&format!("v{i}")), "{path}");
        }
        let shared = files
            .iter()
            .find(|f| f.file_path == "semantics/views/shared.view.yml")
            .expect("shared");
        assert_eq!(
            shared.yaml,
            body_of("second"),
            "row order is kept, so the later row still wins the shared path"
        );
        let paths: Vec<&str> = files.iter().map(|f| f.file_path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "files come back sorted by path");
    }
}
