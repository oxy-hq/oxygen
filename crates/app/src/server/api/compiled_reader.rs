//! Hybrid Postgres-first reader for endpoints on the compile boundary.
//! Every public fn returns:
//!
//! * `Ok(Some(...))` — workspace is promoted on its default branch; here
//!   are the rows.
//! * `Ok(None)` — caller should fall through to the legacy FS path.
//!   Reasons: branch is non-default (IDE draft mode), no promoted revision
//!   exists yet, or a transient DB error.
//! * `Err(...)` — programmer-error DB failure surfaced from a successful
//!   `current_revision_id` lookup (rare). Connect failures are
//!   intentionally downgraded to `Ok(None)` so a Postgres hiccup can never
//!   blank the IDE sidebar.
//!
//! Resolution order on every call:
//!
//! 1. If the workspace is the nil-UUID local/single-instance workspace →
//!    `Ok(None)`. Local mode has the live working copy on disk and no
//!    compile-on-save, so it always reads FS.
//! 2. **IDE / single-process only:** if `branch_hint` is `Some(name)` and that
//!    name is not the workspace's default branch → `Ok(None)`. Non-default
//!    branches are drafts; the FS working copy (with uncommitted edits) is
//!    freshest. A stateless `serve` replica SKIPS this gate — it has no working
//!    copy, so it serves the latest promoted revision regardless of branch.
//! 3. Read `workspaces.current_revision_id`. If null → `Ok(None)`.
//! 4. Query the per-entity table keyed by that revision_id.
//!
//! A workspace **preview** never enters this resolution. The workspace
//! middleware pins a preview's revision for a request that asked for one (see
//! `server::previews`), and the pin short-circuits everything above; nothing
//! here consults the preview registry. That is what keeps background work —
//! which has no request, so no pin — on the promoted revision.

use crate::server::role_manifest::current_process_role;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

tokio::task_local! {
    /// Request-scoped revision pin. Set once by `workspace_middleware` for the
    /// duration of an HTTP request (via [`with_pinned_revision`]); read at the
    /// single resolver choke-point [`open_compiled_revision`]. With it set,
    /// every reader in one request shares ONE revision — so a promotion landing
    /// mid-request can never produce a torn read (config from revision N, app
    /// from N+1). It is **read-consistency request context**, not pipeline
    /// state: absent outside an HTTP request (background tasks, spawned
    /// sub-tasks), the readers fall back to per-call resolution unchanged.
    ///
    /// The inner `Option<Uuid>` distinguishes "pinned to revision X" (`Some`)
    /// from "pinned to no revision — read FS" (`None`); the task-local being
    /// *present at all* is what makes a reader trust the pin instead of
    /// re-resolving.
    static PINNED_REVISION: Option<Uuid>;
}

/// Run `fut` with the request's revision pinned. `pinned` is the result of a
/// single [`resolve_request_revision`] call at request entry.
pub async fn with_pinned_revision<F, T>(pinned: Option<Uuid>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    PINNED_REVISION.scope(pinned, fut).await
}

/// Resolve the one revision this request should read, applying the full
/// branch / promotion / local-mode / DB-health logic exactly once. The caller
/// (`workspace_middleware`) stashes the result via [`with_pinned_revision`] so
/// every downstream reader is consistent. `None` → the request reads the
/// filesystem everywhere (non-default branch, unpromoted, local mode, or a DB
/// hiccup), matching the per-call fallback contract.
pub async fn resolve_request_revision(
    workspace_id: Uuid,
    branch_hint: Option<&str>,
) -> Option<Uuid> {
    let (db, candidate) = match open_compiled_revision(workspace_id, branch_hint).await {
        Ok(Some(pair)) => pair,
        _ => return None,
    };

    // Fail-safe: never pin a revision whose compiled config won't deserialise
    // into the runtime `Config` — that revision 503s every request for this
    // workspace. Prefer the most recent prior revision that DOES deserialise
    // (last-known-good), degrading one bad promote to slightly-stale-but-working
    // data instead of an outage. The compile-time round-trip gate keeps
    // `current_revision_id` good going forward; this covers revisions promoted
    // before the gate existed (the #2520 transition) and any gate bypass.
    if revision_config_loads(&db, candidate).await {
        return Some(candidate);
    }
    tracing::warn!(
        workspace_id = %workspace_id,
        broken_revision = %candidate,
        "pinned revision config does not deserialise; searching for last-known-good"
    );
    last_known_good_revision(&db, workspace_id, candidate).await
}

/// Whether `revision_id` can be pinned for a request: its compiled config (if
/// any) deserialises into the runtime `Config`. The same check
/// [`resolve_request_revision`] makes before pinning the promoted revision,
/// exposed for the preview pin, which must refuse a broken preview outright —
/// the last-known-good walk that rescues a broken promoted revision would hand
/// a preview request MAIN's content.
pub async fn revision_is_servable(db: &DatabaseConnection, revision_id: Uuid) -> bool {
    revision_config_loads(db, revision_id).await
}

/// Process-local memo of "revision R's compiled config deserialises into
/// `Config`". Revision IDs are immutable, so the answer is stable for a
/// revision's lifetime; this keeps the happy path O(1) after the first check.
fn config_validity_cache() -> &'static Mutex<HashMap<Uuid, bool>> {
    static CACHE: OnceLock<Mutex<HashMap<Uuid, bool>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True when this revision's compiled config deserialises into the runtime
/// `Config` (i.e. the request hot path can serve it). A revision with NO config
/// row is treated as valid — there's no config to 503 on, and the existing
/// FS/NeedsRecompile fallthrough handles it; we only walk back on an actual
/// deserialise failure. DB errors are treated as valid (fail open to the
/// existing behaviour rather than blanking a workspace on a transient hiccup).
async fn revision_config_loads(db: &DatabaseConnection, revision_id: Uuid) -> bool {
    if let Some(v) = config_validity_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(&revision_id).copied())
    {
        return v;
    }
    let valid = match load_config_value(db, revision_id).await {
        Ok(Some(value)) => serde_json::from_value::<oxy::config::model::Config>(value).is_ok(),
        Ok(None) => true,
        Err(e) => {
            // Fail OPEN (don't blank a workspace on a transient hiccup) but do
            // NOT cache it: memoising `true` here would permanently mark a
            // genuinely-broken revision valid for this process, defeating the
            // last-known-good fallback whose whole job is preventing 503s.
            // Re-evaluate on the next request instead. (#2524 review)
            tracing::warn!(
                ?e, %revision_id,
                "config validity check: DB error; treating as valid for this request only (not cached)"
            );
            return true;
        }
    };
    if let Ok(mut c) = config_validity_cache().lock() {
        c.insert(revision_id, valid);
    }
    valid
}

/// Walk recent `ready` `main` revisions newest-first and return the first whose
/// compiled config deserialises. Bounded scan so a long broken history can't
/// turn one request into a long sweep. `None` → no good revision found (caller
/// falls through to FS / NeedsRecompile, matching today's behaviour).
async fn last_known_good_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    exclude: Uuid,
) -> Option<Uuid> {
    const SCAN_LIMIT: u64 = 10;
    let rows = entity::revisions::Entity::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
        .filter(entity::revisions::Column::Kind.eq("main"))
        .filter(entity::revisions::Column::Status.eq("ready"))
        .order_by_desc(entity::revisions::Column::FinishedAt)
        .limit(SCAN_LIMIT)
        .all(db)
        .await
        .unwrap_or_default();
    for r in rows {
        if r.revision_id == exclude {
            continue;
        }
        if revision_config_loads(db, r.revision_id).await {
            tracing::info!(
                workspace_id = %workspace_id,
                good_revision = %r.revision_id,
                "serving last-known-good compiled revision (current revision config is unreadable)"
            );
            return Some(r.revision_id);
        }
    }
    None
}

/// Single-entity resolver result. Carries the full `definition` JSONB
/// so the runtime can deserialize into its strict type.
#[derive(Debug, Clone)]
pub struct CompiledArtifact {
    pub file_path: String,
    pub name: String,
    pub definition: Value,
    /// When set, an S3 object holds the canonical body for this
    /// artifact (semantic views + topics only). Materialisers should
    /// prefer the blob over `definition` to keep tablespace cost
    /// bounded for large semantic models. Falls back to `definition`
    /// when the blob is missing / transport errors.
    pub compiled_sql_blob_key: Option<String>,
}

/// Verified-query (`.sql`) resolver result. Unlike the YAML entities a
/// verified query has no parsed `definition` — its body IS the raw SQL
/// text, carried verbatim alongside the content hash the compile worker
/// recorded (`content_sha256`). The hash lets a caller verify integrity
/// after a Postgres/S3 round-trip (the content-addressing invariant).
#[derive(Debug, Clone)]
pub struct CompiledVerifiedQuery {
    pub file_path: String,
    pub content_sha256: String,
    pub content: String,
}

impl CompiledVerifiedQuery {
    /// Re-hash the carried body and compare to the `content_sha256` the
    /// compile worker recorded. A mismatch means the body was corrupted on
    /// the Postgres/S3 round-trip — the content-addressing integrity check
    /// from the compile-complete design (a verifier the loud serve-side
    /// `WorkspaceFs` impl can use to refuse a corrupt artifact rather than
    /// silently serve it). The hash format mirrors `oxy_compile`'s
    /// `compile_verified_query`: lowercase-hex SHA-256 of the raw bytes.
    pub fn integrity_ok(&self) -> bool {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.content.as_bytes());
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        actual == self.content_sha256
    }
}

/// [`list_analytics_agents`] against a revision the caller already holds.
///
async fn conn() -> Result<DatabaseConnection, sea_orm::DbErr> {
    oxy::database::client::establish_connection()
        .await
        .map_err(|e| sea_orm::DbErr::Conn(sea_orm::RuntimeErr::Internal(e.to_string())))
}

/// Resolve the workspace's compiled `config.yml`. The
/// `workspace_compiled_configs` table mirrors the top-level shape of
/// the runtime `Config` struct in JSONB columns; callers merge them
/// back into one JSON object and deserialise with `serde_json::from_value`.
///
/// Caller is responsible for setting `Config.workspace_path` after
/// deserialisation — it's `#[serde(skip)]` so the round-trip leaves
/// it empty.
pub async fn resolve_workspace_config(
    workspace_id: Uuid,
    branch_hint: Option<&str>,
) -> Result<Option<Value>, sea_orm::DbErr> {
    Ok(
        resolve_workspace_config_with_revision(workspace_id, branch_hint)
            .await?
            .map(|(value, _)| value),
    )
}

/// [`resolve_workspace_config`] carrying the revision the value came from, so a
/// caller can record it on the manager and later read the same revision instead
/// of re-resolving one. Re-resolution is what leaves the six non-HTTP entry
/// points without the last-known-good walk.
pub async fn resolve_workspace_config_with_revision(
    workspace_id: Uuid,
    branch_hint: Option<&str>,
) -> Result<Option<(Value, Uuid)>, sea_orm::DbErr> {
    let Some((db, revision_id)) = open_compiled_revision(workspace_id, branch_hint).await? else {
        return Ok(None);
    };
    Ok(load_config_value(&db, revision_id)
        .await?
        .map(|value| (value, revision_id)))
}

/// [`resolve_workspace_config`] against a revision the caller already holds.
pub async fn resolve_workspace_config_at(
    revision_id: Uuid,
) -> Result<Option<Value>, sea_orm::DbErr> {
    load_config_value(&conn().await?, revision_id).await
}

/// Load and merge the compiled config for a specific revision into the single
/// top-level object `config.yml` deserialises from. Uses the SAME merge as the
/// compile-time gate (`oxy_compile::merge_compiled_config`) so the shape the
/// gate validated and the shape the reader serves can never drift.
async fn load_config_value(
    db: &DatabaseConnection,
    revision_id: Uuid,
) -> Result<Option<Value>, sea_orm::DbErr> {
    let Some(row) = entity::workspace_compiled_configs::Entity::find_by_id(revision_id)
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let cfg = oxy_compile::CompiledConfig {
        databases: row.databases,
        models: row.models,
        integrations: row.integrations,
        repositories: row.repositories,
        builder_agent: row.builder_agent,
        mcp: row.mcp,
        other: row.other,
    };
    Ok(Some(oxy_compile::merge_compiled_config(&cfg)))
}

/// [`resolve_reconcile_config`] against a revision the caller already holds.
pub async fn resolve_reconcile_config_at(
    revision_id: Uuid,
) -> Result<Option<Value>, sea_orm::DbErr> {
    Ok(entity::reconcile_configs::Entity::find_by_id(revision_id)
        .one(&conn().await?)
        .await?
        .map(|m| m.definition))
}

/// Single semantic-view resolver, keyed by `name`.
pub async fn resolve_semantic_view(
    workspace_id: Uuid,
    branch_hint: Option<&str>,
    file_path: &str,
) -> Result<Option<CompiledArtifact>, sea_orm::DbErr> {
    let Some((db, revision_id)) = open_compiled_revision(workspace_id, branch_hint).await? else {
        return Ok(None);
    };
    // Key by `file_path`, NOT the PK `name`. Callers (the IDE preview) only have
    // the workspace-relative path; the row's `name` is the YAML `name:` field
    // (e.g. `oxymart`), which never equals the path. Looking up by name with a
    // path always missed → the read fell through to the working-copy FS, which a
    // stateless serve replica doesn't have. One file → one row, so `.one()` is
    // safe.
    let row = entity::semantic_views::Entity::find()
        .filter(entity::semantic_views::Column::RevisionId.eq(revision_id))
        .filter(entity::semantic_views::Column::FilePath.eq(file_path))
        .one(&db)
        .await?;
    Ok(row.map(|m| CompiledArtifact {
        file_path: m.file_path,
        name: m.name,
        definition: m.definition,
        compiled_sql_blob_key: m.compiled_sql_blob_key,
    }))
}

/// Single semantic-topic resolver, keyed by workspace-relative `file_path`
/// (the row's PK `name` is the YAML `name:` field, which the caller doesn't have
/// — see `resolve_semantic_view`).
pub async fn resolve_semantic_topic(
    workspace_id: Uuid,
    branch_hint: Option<&str>,
    file_path: &str,
) -> Result<Option<CompiledArtifact>, sea_orm::DbErr> {
    let Some((db, revision_id)) = open_compiled_revision(workspace_id, branch_hint).await? else {
        return Ok(None);
    };
    let row = entity::semantic_topics::Entity::find()
        .filter(entity::semantic_topics::Column::RevisionId.eq(revision_id))
        .filter(entity::semantic_topics::Column::FilePath.eq(file_path))
        .one(&db)
        .await?;
    Ok(row.map(|m| CompiledArtifact {
        file_path: m.file_path,
        name: m.name,
        definition: m.definition,
        compiled_sql_blob_key: m.compiled_sql_blob_key,
    }))
}

/// Single pipeline resolver, keyed by `file_path`. `oxy_compile::walker`
/// derives a fallback `name` from the file stem, so name-keyed lookup misses
/// every pipeline whose `name:` differs from its path — we filter on
/// `file_path`, same reasoning as `resolve_semantic_view`. One file → one row.
///
/// Cross-tenant containment comes for free: `revision_id` belongs to exactly
/// one workspace, so a `pipeline_ref` can only ever address a row inside the
/// caller's own promoted revision.
pub async fn resolve_pipeline(
    workspace_id: Uuid,
    branch_hint: Option<&str>,
    file_path: &str,
) -> Result<Option<CompiledArtifact>, sea_orm::DbErr> {
    let Some((db, revision_id)) = open_compiled_revision(workspace_id, branch_hint).await? else {
        return Ok(None);
    };
    let row = entity::airway_pipelines::Entity::find()
        .filter(entity::airway_pipelines::Column::RevisionId.eq(revision_id))
        .filter(entity::airway_pipelines::Column::FilePath.eq(file_path))
        .one(&db)
        .await?;
    Ok(row.map(|m| CompiledArtifact {
        file_path: m.file_path,
        name: m.name,
        definition: m.definition,
        compiled_sql_blob_key: None,
    }))
}

/// Single verified-query resolver, keyed by `file_path`. Verified queries
/// are referenced by their workspace-relative path (the agent `context:`
/// glob discovers them on disk by path; on the boundary that same path is
/// the row's PK alongside `revision_id`), so unlike the named entities we
/// look up by `file_path` — mirroring `resolve_app`.
///
/// Shared gate consulted at the top of every public reader. Returns
/// `Some((db, revision_id))` when the request should be served from
/// Postgres, `None` when the caller should fall through to FS.
///
/// Returns `Ok(None)` (not `Err`) for non-default branch, DB connect
/// failure, or workspace-not-promoted. We never want the IDE sidebar to
/// blank because the compile boundary is unhappy — FS is always the safety net.
async fn open_compiled_revision(
    workspace_id: Uuid,
    branch_hint: Option<&str>,
) -> Result<Option<(DatabaseConnection, Uuid)>, sea_orm::DbErr> {
    // Local / single-instance mode (the nil-UUID workspace) always has the live
    // working copy on disk and there's no compile-on-save, so serve from the
    // filesystem — otherwise an on-disk edit to a promoted workspace wouldn't
    // show until the next manual compile. The compile boundary exists for the
    // stateless cloud fleet, which has no working copy.
    if workspace_id == oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID {
        return Ok(None);
    }
    // Request-scoped pin: `workspace_middleware` already ran the branch /
    // promotion / local checks once and stashed the result, so trust it and
    // skip the re-resolve. Every reader in the request thus shares one revision
    // (no torn reads). `Some(None)` means "this request reads FS everywhere".
    if let Ok(pinned) = PINNED_REVISION.try_with(|p| *p) {
        let Some(revision_id) = pinned else {
            return Ok(None);
        };
        return match oxy::database::client::establish_connection().await {
            Ok(db) => Ok(Some((db, revision_id))),
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "compiled_reader: DB connect failed (pinned); falling to FS"
                );
                Ok(None)
            }
        };
    }
    let db = match oxy::database::client::establish_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                ?e,
                "compiled_reader: DB connect failed; falling through to FS"
            );
            return Ok(None);
        }
    };
    // Branch gate — only on a node that HAS a working copy (IDE / single
    // process). There, a non-default branch is a draft and the on-disk working
    // copy (with uncommitted edits) is freshest, so fall through to FS. A
    // stateless `serve` replica has NO working copy: "fall back to FS" there
    // degrades to a useless `needs_recompile` 503, and pinning a per-workspace
    // default branch is fragile. So a serve replica skips the gate and serves
    // the latest promoted revision regardless of which branch the FE tagged —
    // the compiled revision is the only (and freshest) thing it can serve.
    // See oxygen-internal#2528.
    let effective_branch = normalize_branch_hint(branch_hint);
    // A replica cannot honour a branch, so a route that accepts one is either
    // misclassified or is asking a question this pod cannot answer. The reply is
    // the promoted default-branch revision either way — the caller asked for a
    // feature branch and gets `main` with no error, which is the one shape that
    // looks like working software. Count it, so a misclassified route shows up
    // on a dashboard rather than as "the IDE isn't showing my edits".
    //
    // The predicate is "does this process own workspace files", NOT
    // `role == Serve`. They are not the same set: `role_owns_workspace_files`
    // is `Ide | All`, so a WORKER is equally diskless — and under the old check
    // a worker took the branch-gate arm below, falling through to a working
    // copy it does not have. Nothing had reported it because workers rarely
    // carry a branch hint, which is exactly why it would have been found late.
    let owns_files = oxy::workspace_fs_probe::process_owns_workspace_files();
    if !owns_files
        && let Some(branch) = effective_branch
        && !is_default_branch(&db, workspace_id, branch).await
    {
        BRANCH_HINTS_DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::warn!(
            workspace_id = %workspace_id,
            branch,
            role = ?current_process_role(),
            "compiled_reader: this process holds no working copy, so it cannot honour \
             a branch hint; serving the promoted default-branch revision. A route that \
             takes `?branch=` belongs on the ide (role_manifest IdeOnly)."
        );
    }
    if owns_files
        && let Some(branch) = effective_branch
        && !is_default_branch(&db, workspace_id, branch).await
    {
        tracing::debug!(
            workspace_id = %workspace_id,
            branch,
            "compiled_reader: non-default branch on a working-copy node — using FS"
        );
        return Ok(None);
    }
    let row = entity::workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await?;
    let Some(revision_id) = row.and_then(|w| w.current_revision_id) else {
        return Ok(None);
    };
    Ok(Some((db, revision_id)))
}

/// How many times a serve replica has been handed a non-default branch hint it
/// cannot honour, since startup.
///
/// The static counterpart of the route classification: `role_manifest` says
/// which routes should reach a replica, and this says which ones actually did
/// while carrying a question only the ide can answer. Non-zero means a route
/// takes `?branch=` and is not `IdeOnly` — plan rule 6, measured rather than
/// reviewed.
static BRANCH_HINTS_DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`BRANCH_HINTS_DROPPED`]. Readable so a fleet canary can assert zero.
pub fn branch_hints_dropped() -> u64 {
    BRANCH_HINTS_DROPPED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Treat an empty `branch` query param the same as `None`. Most FE calls
/// omit the branch from the URL, so they arrive at the middleware as
/// `Some("")` after axum's query parser. Previously this fell through to
/// FS (which on a serve replica means a 500 — no working copy). Empty
/// means "default branch": let `open_compiled_revision` resolve it from
/// the workspace row instead. Regression context: oxy-hq/oxygen-internal#1619.
fn normalize_branch_hint(branch_hint: Option<&str>) -> Option<&str> {
    branch_hint.filter(|b| !b.is_empty())
}

/// Returns true when `branch` is the workspace's recorded default
/// branch. Detected lazily and cached per-process by
/// [`crate::server::default_branch::resolve_default_branch`].
///
/// Only called for branch-hinted (IDE) requests — non-IDE callers
/// pass `None` and bypass this entire check. On any resolver failure
/// (workspace not yet on disk, git lookup errored) we return `false`,
/// which routes the IDE request to FS. That matches the design
/// premise: a non-classifiable branch is closer to "user is editing
/// something we can't validate" than "the user is on main", and FS
/// reading the working copy is always safe. Returning `true` here
/// would serve the promoted main revision to a feature-branch user
/// whenever git resolution hiccups — the opposite of what
/// branch-aware reading is for.
async fn is_default_branch(db: &DatabaseConnection, workspace_id: Uuid, branch: &str) -> bool {
    match crate::server::default_branch::resolve_default_branch(db, workspace_id).await {
        Some(default) => branch == default,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_branch_hint_strips_empty() {
        // An empty `?branch=` from the FE must NOT trigger the non-default
        // branch fallthrough — empty means "use the workspace default".
        assert_eq!(normalize_branch_hint(None), None);
        assert_eq!(normalize_branch_hint(Some("")), None);
        assert_eq!(normalize_branch_hint(Some("main")), Some("main"));
        assert_eq!(normalize_branch_hint(Some("feat/x")), Some("feat/x"));
        // We don't trim whitespace; a literal space is technically a legal
        // git branch name, so we treat it as a real branch hint.
        assert_eq!(normalize_branch_hint(Some(" ")), Some(" "));
    }

    #[test]
    fn verified_query_integrity_detects_corruption() {
        let content = "SELECT 1 /* oxy: verified */";
        // Hash the body the same way `oxy_compile::compile_verified_query`
        // does, so a faithful round-trip verifies.
        let sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(content.as_bytes());
            h.finalize()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>()
        };
        let good = CompiledVerifiedQuery {
            file_path: "example_sql/answer.sql".to_string(),
            content_sha256: sha,
            content: content.to_string(),
        };
        assert!(
            good.integrity_ok(),
            "a faithfully round-tripped body verifies"
        );

        // Same recorded hash, different bytes → corruption is caught. This
        // case is non-circular: the hash is fixed, only `content` changes.
        let corrupted = CompiledVerifiedQuery {
            content: "SELECT 2 /* tampered */".to_string(),
            ..good.clone()
        };
        assert!(
            !corrupted.integrity_ok(),
            "a body whose bytes no longer match the recorded hash is rejected"
        );
    }
}
