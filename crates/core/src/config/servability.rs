//! Whether a compiled revision can be served by this binary, and which
//! revision a request may be pinned to by id.
//!
//! - **Servability:** [`revision_is_servable`] / [`revision_config_loads`] ask
//!   whether a revision's compiled `config.yml` ([`load_config_value`])
//!   deserialises into this binary's [`Config`](crate::config::model::Config).
//!   It is evaluated per server version, so it cannot be stamped at compile
//!   time. The answer is memoised in one process-wide cache, which every
//!   caller shares: the pin check below, and `oxy_app_core`'s revision
//!   resolver (the promoted revision and its last-known-good walk).
//! - **Pinning:** a `ready` `staging`/`main` revision of the request's own
//!   workspace ([`check_revision`], the rule a custom-app publish applies too)
//!   whose compiled config the runtime can load ([`check_pinnable`], for a
//!   workspace preview header).
//!
//! Here, beside `Config` and compiled storage, because two domains apply the
//! pin rule (a custom app's publish and a workspace preview's header,
//! `oxy-previews`) and neither may depend on the other, nor on `oxy-app-core`
//! for it (`internal-docs/crate-tiers.md`, stages A and C). It moved down from
//! `oxy_app_core::revision_pin` and the servability half of the app-core
//! revision resolver on 2026-11-27; `oxy_app_core::revision_pin` and
//! `oxy_custom_apps::staging_pin` re-export it under its old names for one
//! release. It picks a revision; it reads no artifact.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::Value;
use uuid::Uuid;

/// Whether `revision_id` may be pinned for a request on `workspace_id`: a
/// `ready` `staging`/`main` revision of that workspace (the publish-time check)
/// whose compiled config the runtime can load. For a caller that pins by id per
/// request — a workspace preview header.
///
/// The config check matters more here than at publish: a request whose pinned
/// config will not load falls back to the last-known-good MAIN revision, so the
/// preview would answer with main's content under a preview label.
pub async fn check_pinnable(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    revision_id: Uuid,
) -> Result<entity::revisions::Model, PinRefusal> {
    let rev = entity::revisions::Entity::find_by_id(revision_id)
        .one(db)
        .await
        .map_err(|e| PinRefusal::Db(e.to_string()))?
        .ok_or(PinRefusal::NotFound(revision_id))?;
    check_revision(&rev, workspace_id)?;
    if !revision_is_servable(db, revision_id).await {
        return Err(PinRefusal::Unservable(revision_id));
    }
    Ok(rev)
}

/// Why a pin was refused: a publish's `semantic_revision_id`, or a workspace
/// preview header's revision. For a publish, every variant is a 400 or 422 the
/// publisher can act on.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PinRefusal {
    #[error(
        "a pinned semantic revision is staging-only: it cannot be combined with --promote. \
         Merge the branch so main compiles, then promote the build"
    )]
    WithPromote,
    #[error("semantic revision {0} not found")]
    NotFound(Uuid),
    #[error(
        "semantic revision {revision} belongs to workspace {actual}, not this app's workspace {expected}"
    )]
    OtherWorkspace {
        revision: Uuid,
        expected: Uuid,
        actual: Uuid,
    },
    #[error("semantic revision {revision} is {status}, not ready — wait for the compile to finish")]
    NotReady { revision: Uuid, status: String },
    #[error(
        "semantic revision {revision} has kind {kind}; only a staging or main revision can be pinned"
    )]
    WrongKind { revision: Uuid, kind: String },
    #[error("semantic revision {0}'s compiled config does not load on this server")]
    Unservable(Uuid),
    #[error("database error: {0}")]
    Db(String),
}

/// The pure half of the pin check: the revision belongs to `project_id` and is
/// a `ready` `staging`/`main` revision. A custom-app publish
/// (`oxy_custom_apps::staging_pin::validate_pin_for_publish`) applies it alone;
/// [`check_pinnable`] adds that the compiled config loads.
pub fn check_revision(rev: &entity::revisions::Model, project_id: Uuid) -> Result<(), PinRefusal> {
    if rev.workspace_id != project_id {
        return Err(PinRefusal::OtherWorkspace {
            revision: rev.revision_id,
            expected: project_id,
            actual: rev.workspace_id,
        });
    }
    if rev.kind != "staging" && rev.kind != "main" {
        return Err(PinRefusal::WrongKind {
            revision: rev.revision_id,
            kind: rev.kind.clone(),
        });
    }
    if rev.status != "ready" {
        return Err(PinRefusal::NotReady {
            revision: rev.revision_id,
            status: rev.status.clone(),
        });
    }
    Ok(())
}

/// Whether `revision_id` can be pinned for a request: its compiled config (if
/// any) deserialises into the runtime `Config`. The same check the app-core
/// revision resolver makes before pinning the promoted revision, exposed for
/// the preview pin, which must refuse a broken preview outright — the
/// last-known-good walk that rescues a broken promoted revision would hand a
/// preview request MAIN's content.
pub async fn revision_is_servable(db: &DatabaseConnection, revision_id: Uuid) -> bool {
    revision_config_loads(db, revision_id).await
}

/// Process-local memo of "revision R's compiled config deserialises into
/// `Config`". Revision IDs are immutable, so the answer is stable for a
/// revision's lifetime; this keeps the happy path O(1) after the first check.
///
/// One instance per process: the pin check here and the app-core resolver's
/// promoted-revision check and last-known-good walk all read it through
/// [`revision_config_loads`]. Never copy it into another crate — a second
/// cache would let the two disagree about the same revision.
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
pub async fn revision_config_loads(db: &DatabaseConnection, revision_id: Uuid) -> bool {
    if let Some(v) = config_validity_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(&revision_id).copied())
    {
        return v;
    }
    let valid = match load_config_value(db, revision_id).await {
        Ok(Some(value)) => serde_json::from_value::<crate::config::model::Config>(value).is_ok(),
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

/// Load and merge the compiled config for a specific revision into the single
/// top-level object `config.yml` deserialises from. Uses the SAME merge as the
/// compile-time gate (`oxy_compile::merge_compiled_config`) so the shape the
/// gate validated and the shape the reader serves can never drift.
pub async fn load_config_value(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rev(workspace_id: Uuid, kind: &str, status: &str) -> entity::revisions::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::revisions::Model {
            revision_id: Uuid::new_v4(),
            workspace_id,
            git_sha: "abc".into(),
            branch: Some("feature".into()),
            schema_version: 1,
            status: status.into(),
            kind: kind.into(),
            owner_user_id: None,
            compiler_version: "x".into(),
            started_at: now,
            finished_at: Some(now),
            file_count_seen: 0,
            file_count_compiled: 0,
            file_count_failed: 0,
            error_summary: None,
        }
    }

    #[test]
    fn a_ready_staging_or_main_revision_of_the_workspace_is_pinnable() {
        let ws = Uuid::new_v4();
        assert_eq!(check_revision(&rev(ws, "staging", "ready"), ws), Ok(()));
        assert_eq!(check_revision(&rev(ws, "main", "ready"), ws), Ok(()));
    }

    #[test]
    fn other_workspace_draft_and_unready_are_refused() {
        let ws = Uuid::new_v4();
        assert!(matches!(
            check_revision(&rev(Uuid::new_v4(), "staging", "ready"), ws),
            Err(PinRefusal::OtherWorkspace { .. })
        ));
        assert!(matches!(
            check_revision(&rev(ws, "draft", "ready"), ws),
            Err(PinRefusal::WrongKind { .. })
        ));
        assert!(matches!(
            check_revision(&rev(ws, "staging", "compiling"), ws),
            Err(PinRefusal::NotReady { .. })
        ));
    }

    #[test]
    fn the_promote_refusal_names_the_way_forward() {
        let msg = PinRefusal::WithPromote.to_string();
        assert!(msg.contains("staging-only"), "{msg}");
        assert!(msg.contains("Merge the branch"), "{msg}");
    }

    /// The validity cache is one instance: an answer recorded once is what
    /// both the pin check ([`revision_is_servable`]) and the resolver's check
    /// ([`revision_config_loads`], which `oxy_app_core`'s promoted-revision
    /// pin and last-known-good walk call) return, without asking the database.
    /// The connection is disconnected, which sea-orm refuses to query, so a
    /// read that missed the cache could not have answered `false`.
    #[tokio::test]
    async fn the_servability_answer_comes_from_one_shared_cache() {
        let db = DatabaseConnection::default();
        let broken = Uuid::new_v4();
        config_validity_cache()
            .lock()
            .unwrap()
            .insert(broken, false);
        assert!(!revision_is_servable(&db, broken).await);
        assert!(!revision_config_loads(&db, broken).await);
        assert!(std::ptr::eq(
            config_validity_cache(),
            config_validity_cache()
        ));
    }
}
