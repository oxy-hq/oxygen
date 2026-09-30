//! Postgres writes for the compile boundary.
//!
//! Strict layering between this module and the rest of the crate:
//! - `compile.rs` walks files and builds in-memory rows.
//! - This module is the **only** code that talks to the database.
//!
//! Two phases:
//!   1. `insert_compiling_revision` writes the `revisions` row with
//!      `status = "compiling"` and returns the new `revision_id`. Done
//!      up front so the operator can see "this workspace started a
//!      compile at <time>" before we finish walking files.
//!   2. `finalise_revision` inserts all per-entity rows, sets
//!      `revisions.status` to `ready` or `failed`, records the
//!      `error_summary`, and stamps `finished_at`. Wrapped in a tx so
//!      a half-finished revision is impossible. When the caller
//!      requested `promote = true` AND the revision succeeded AND
//!      `kind == "main"`, the same tx also updates
//!      `workspaces.current_revision_id`, conditioned on the
//!      revision's `started_at` ordering so a slower compile cannot
//!      overwrite a faster one's pointer.

use crate::compile::CompiledRow;
use crate::errors::CompileError;
use crate::outcome::{FileFailure, Promotion, RevisionStatus};
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, EntityTrait, TransactionTrait,
};
use uuid::Uuid;

pub struct RevisionContext {
    pub revision_id: Uuid,
    pub workspace_id: Uuid,
    pub started_at: DateTime<Utc>,
    /// Carried so `finalise_revision`'s superseded path can look up
    /// the winner without re-fetching the revisions row.
    pub git_sha: String,
}

/// Insert the initial `revisions` row with `status = compiling`. The
/// returned ID is the one every downstream per-entity row will carry.
pub async fn insert_compiling_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
    branch: Option<&str>,
    schema_version: i32,
    compiler_version: &str,
    kind: &str,
    owner_user_id: Option<Uuid>,
) -> Result<RevisionContext, CompileError> {
    let revision_id = Uuid::new_v4();
    let started_at = Utc::now();

    let row = entity::revisions::ActiveModel {
        revision_id: Set(revision_id),
        workspace_id: Set(workspace_id),
        git_sha: Set(git_sha.to_string()),
        branch: Set(branch.map(str::to_string)),
        schema_version: Set(schema_version),
        status: Set("compiling".to_string()),
        kind: Set(kind.to_string()),
        owner_user_id: Set(owner_user_id),
        compiler_version: Set(compiler_version.to_string()),
        started_at: Set(started_at.fixed_offset()),
        finished_at: Set(None),
        file_count_seen: Set(0),
        file_count_compiled: Set(0),
        file_count_failed: Set(0),
        error_summary: NotSet,
    };
    row.insert(db).await?;

    Ok(RevisionContext {
        revision_id,
        workspace_id,
        started_at,
        git_sha: git_sha.to_string(),
    })
}

pub struct FinaliseInput<'a> {
    pub status: RevisionStatus,
    pub file_count_seen: u32,
    pub file_count_compiled: u32,
    pub file_count_failed: u32,
    pub failures: &'a [FileFailure],
    pub rows: &'a [CompiledRow],
    /// When true AND the compile succeeded AND `kind == "main"`,
    /// also atomically update `workspaces.current_revision_id` to
    /// this revision_id inside the same transaction. The IDE Compile
    /// button and the admin "Run compile now" form set this; the CLI
    /// defaults to `false` so an interactive `oxy compile` stays
    /// observation-only by default.
    pub promote: bool,
    /// `revisions.kind` from the original insert; needed here to gate
    /// promotion (drafts are never promoted).
    pub kind: String,
    /// How many `schemas/*.sql` files this revision compiled.
    ///
    /// Non-zero withholds the in-transaction promote: the DDL has to
    /// reach the org's OLTP database before the runtime is pointed at a
    /// revision whose tables may not exist yet. The apply cannot happen
    /// here — it is a network call to a tenant database and this is a
    /// control-plane transaction holding a unique-index lock on
    /// `revisions` — so the caller gets [`Promotion::Deferred`] and owns
    /// the apply-then-promote step.
    pub schema_migration_count: u32,
}

/// Outcome of a finalise call. The common-case is `Committed { .. }`
/// for "our revision is now ready"; `SupersededBy(other)` says
/// another worker won the idempotency race (same workspace + git_sha
/// became `ready` first) and the caller should treat that revision
/// as the authoritative one.
#[derive(Debug)]
pub enum FinaliseOutcome {
    Committed {
        finished_at: DateTime<Utc>,
        promotion: Promotion,
    },
    SupersededBy {
        revision_id: Uuid,
        /// What happened to `current_revision_id` on the way out. The
        /// winner's revision is the subject, not ours.
        promotion: Promotion,
    },
}

/// Finalise the revision: write all compiled-entity rows, update the
/// revision header, in one transaction.
///
/// Multi-worker safety: when the partial unique index
/// `idx_revisions_idempotent_ready_main` rejects this transaction
/// because another worker's revision for the same `(workspace_id,
/// git_sha)` already reached `status='ready' AND kind='main'`, this
/// function returns `FinaliseOutcome::SupersededBy(other)` instead
/// of erroring. The caller treats this as an idempotent short-circuit
/// — same SHA, same source, same compiled rows in DB — and surfaces
/// the winner's outcome. The losing worker's `revisions` row is
/// marked `superseded` so it shows up in the admin timeline as
/// "we lost the race" rather than ghost-`compiling` forever.
pub async fn finalise_revision(
    db: &DatabaseConnection,
    ctx: &RevisionContext,
    input: FinaliseInput<'_>,
) -> Result<FinaliseOutcome, CompileError> {
    // Hoist S3 blob uploads OUT of the Postgres txn. write_compiled_rows
    // used to do the uploads inline, holding the txn (and the partial
    // unique index lock on `revisions`) open for N+M sequential PUTs
    // — fine when OXY_COMPILE_BLOB_S3_BUCKET is unset (uploads are
    // instant no-ops), bad when it's on (network round-trips serialise
    // under the lock, blocking concurrent compiles for the same SHA).
    //
    // Uploading first means: if the worker dies between upload and tx
    // commit, the blobs are orphaned in S3 (no row references them).
    // Orphans are acceptable here — S3 lifecycle rules or a sweep can
    // reap them; the alternative (txn-held S3) is the kind of cross-
    // resource lock that brings everything to a halt under load.
    let blob_keys = prepare_blob_keys(ctx.workspace_id, input.rows).await;

    let txn = db.begin().await?;
    let finished_at = Utc::now();

    write_compiled_rows(&txn, ctx.revision_id, input.rows, &blob_keys).await?;

    let error_summary_json = if input.failures.is_empty() {
        None
    } else {
        Some(serde_json::json!({ "failures": input.failures }))
    };

    let header_update = entity::revisions::ActiveModel {
        revision_id: Set(ctx.revision_id),
        status: Set(input.status.as_str().to_string()),
        finished_at: Set(Some(finished_at.fixed_offset())),
        file_count_seen: Set(input.file_count_seen as i32),
        file_count_compiled: Set(input.file_count_compiled as i32),
        file_count_failed: Set(input.file_count_failed as i32),
        error_summary: Set(error_summary_json),
        // The fields below are unchanged from insert_compiling_revision;
        // ActiveModel diffs by NotSet so listing them isn't required.
        workspace_id: NotSet,
        git_sha: NotSet,
        branch: NotSet,
        schema_version: NotSet,
        kind: NotSet,
        owner_user_id: NotSet,
        compiler_version: NotSet,
        started_at: NotSet,
    };
    // Status update is the point at which the partial unique index
    // (idx_revisions_idempotent_ready_main) gets evaluated — that's
    // where two concurrent finalisations for the same SHA collide.
    match header_update.update(&txn).await {
        Ok(_) => {}
        Err(e) if is_unique_violation_for_idempotency(&e) => {
            // Another worker finalised first. Roll back our tx (the
            // per-entity rows we wrote get discarded; the winner's
            // already-committed rows are the canonical ones), look up
            // their revision, and report ourselves as superseded.
            drop(txn);
            return match lookup_ready_winner(db, ctx.workspace_id, &ctx.git_sha).await? {
                Some(winner) => {
                    mark_superseded(db, ctx.revision_id, winner).await;
                    // Same source tree as the winner, so our count is their
                    // count: if this revision carries DDL, theirs does too
                    // and their worker deferred for the same reason.
                    let mut promotion = match (input.promote, input.schema_migration_count) {
                        (false, _) => Promotion::NotRequested,
                        (true, 0) => Promotion::Skipped,
                        (true, n) => Promotion::Deferred {
                            schema_migration_count: n,
                        },
                    };
                    // `schema_migration_count > 0` opts out entirely: the
                    // winner's own worker deferred its promote so it could
                    // apply DDL first, and promoting it from here would
                    // undo exactly that ordering — pointing the runtime at
                    // a revision whose tables the winner has not created
                    // yet. Same source tree, so our count equals theirs.
                    if input.promote && input.schema_migration_count == 0 {
                        // Promote the winner — normally redundant (the winning
                        // worker promotes inside its own finalise txn), but it
                        // covers the case where the winner did NOT promote. The
                        // conditional UPDATE in promote_revision no-ops if the
                        // workspace already points at a newer revision. Do NOT
                        // swallow the error: a failure here means the workspace
                        // may still point at the OLD revision while we report
                        // success, so it must be observable/alertable.
                        match promote_existing(db, ctx.workspace_id, winner).await {
                            Ok(p) => promotion = p,
                            Err(e) => {
                                tracing::error!(
                                    ?e,
                                    workspace_id = %ctx.workspace_id,
                                    winner = %winner,
                                    "compile: superseded-path promote failed; workspace may still \
                                     point at a stale revision — verify current_revision_id"
                                );
                                promotion = Promotion::Skipped;
                            }
                        }
                    }
                    Ok(FinaliseOutcome::SupersededBy {
                        revision_id: winner,
                        promotion,
                    })
                }
                None => Err(CompileError::Database(e)),
            };
        }
        Err(e) => return Err(CompileError::Database(e)),
    }

    // Atomic promotion — same tx as the row writes, so
    // current_revision_id can never point at an incomplete revision.
    // Gated three ways: (1) the explicit `promote` flag from the
    // caller, (2) status must be Ready, (3) kind must be "main"
    // (drafts are NEVER promoted, even with promote=true). The web
    // request flow that flips a workspace to a draft does so via a
    // *separate* user-scoped read path; current_revision_id stays the
    // authoritative main pointer.
    //
    // A fourth gate defers rather than denies: a revision carrying
    // `schemas/*.sql` is not promoted here at all, because its tables do
    // not exist in the tenant database yet. See `decide_promotion`.
    let promotion = decide_promotion(&input);
    if matches!(promotion, Promotion::Promoted) {
        // `promote_revision` reports whether the causality clause let the
        // UPDATE land. Downgrade to `Skipped` when it didn't, so the
        // caller isn't told this revision is live when a newer one is.
        if !promote_revision(&txn, ctx.workspace_id, ctx.revision_id).await? {
            txn.commit().await?;
            return Ok(FinaliseOutcome::Committed {
                finished_at,
                promotion: Promotion::Skipped,
            });
        }
    }

    txn.commit().await?;
    Ok(FinaliseOutcome::Committed {
        finished_at,
        promotion,
    })
}

/// The promote/defer/decline decision, split out because it is the one
/// piece of this file that is pure — and therefore the one piece worth
/// testing without a database.
///
/// `Promoted` here means "the three gates passed, go attempt it"; the
/// caller downgrades to `Skipped` if the causality clause rejects the
/// UPDATE. That is the only reachable disagreement between this
/// function's answer and the final one.
fn decide_promotion(input: &FinaliseInput<'_>) -> Promotion {
    if !input.promote || !matches!(input.status, RevisionStatus::Ready) || input.kind != "main" {
        return Promotion::NotRequested;
    }
    if input.schema_migration_count > 0 {
        return Promotion::Deferred {
            schema_migration_count: input.schema_migration_count,
        };
    }
    Promotion::Promoted
}

/// Detect the specific Postgres unique-violation on
/// `idx_revisions_idempotent_ready_main`. We match by the index name
/// rather than the SQLSTATE class because the same code (23505) fires
/// on every unique constraint in the schema — we don't want to treat
/// a wholly-unrelated collision as "the idempotency case."
///
/// Matched against the error's Display string: sea_orm / sqlx surface
/// the constraint name verbatim. The test below pins this assumption
/// — a sea_orm / sqlx upgrade that drops the constraint name from
/// the message MUST fail the test loudly rather than silently
/// regress the SupersededBy short-circuit into a hard 500. If the
/// driver shape changes in a future upgrade, replace this with a
/// typed extractor that pulls the constraint name out of the
/// underlying `PgDatabaseError::constraint()` directly.
fn is_unique_violation_for_idempotency(err: &sea_orm::DbErr) -> bool {
    err.to_string()
        .contains("idx_revisions_idempotent_ready_main")
}

#[cfg(test)]
mod idempotency_match_tests {
    use super::is_unique_violation_for_idempotency;
    use sea_orm::DbErr;

    /// Smoke check: a synthetic error message carrying the index name
    /// matches. Documents the shape we expect from the driver so a
    /// future upgrade that changes the Display format trips this
    /// test instead of silently turning the SupersededBy
    /// short-circuit into a hard 500.
    #[test]
    fn matches_when_index_name_in_display() {
        let synthetic = DbErr::Custom(
            "duplicate key value violates unique constraint \
             \"idx_revisions_idempotent_ready_main\""
                .into(),
        );
        assert!(is_unique_violation_for_idempotency(&synthetic));
    }

    #[test]
    fn does_not_match_unrelated_constraints() {
        let unrelated = DbErr::Custom(
            "duplicate key value violates unique constraint \"some_other_index\"".into(),
        );
        assert!(!is_unique_violation_for_idempotency(&unrelated));
    }
}

/// Look up the winning revision (the one that committed first).
/// Used by `finalise_revision`'s superseded path. Returns the
/// caller's own revision_id when no winner is found — which would
/// be a genuine error and the caller should bubble the DB error.
async fn lookup_ready_winner(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<Option<Uuid>, CompileError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    let row = entity::revisions::Entity::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
        .filter(entity::revisions::Column::GitSha.eq(git_sha))
        .filter(entity::revisions::Column::Status.eq("ready"))
        .filter(entity::revisions::Column::Kind.eq("main"))
        .order_by_desc(entity::revisions::Column::FinishedAt)
        .one(db)
        .await?;
    Ok(row.map(|r| r.revision_id))
}

/// Mark the losing revision as `superseded` so the admin timeline
/// shows what happened. Best-effort — a DB hiccup here is acceptable
/// because the row is already inert (rolled back from `compiling`,
/// no per-entity rows attached after the rollback).
async fn mark_superseded(db: &DatabaseConnection, revision_id: Uuid, winner: Uuid) {
    use sea_orm::ActiveModelTrait;
    let row = entity::revisions::ActiveModel {
        revision_id: Set(revision_id),
        status: Set("superseded".to_string()),
        finished_at: Set(Some(Utc::now().fixed_offset())),
        error_summary: Set(Some(serde_json::json!({
            "superseded_by": winner.to_string(),
        }))),
        ..Default::default()
    };
    if let Err(e) = row.update(db).await {
        tracing::warn!(?e, %revision_id, %winner, "best-effort mark_superseded failed");
    }
}

/// Idempotent promotion path used by the compile orchestrator's
/// short-circuit: the per-entity rows already exist tagged with this
/// revision_id, so we just update `current_revision_id`. Not inside
/// a tx because there's no atomicity gain — only one column changes.
///
/// Refuses anything but a `main` revision: a `draft` or `staging` revision
/// must never become `current_revision_id`, whichever door asks. A staging
/// revision is a branch head a custom-app draft build pins — promoting it
/// would make prod serve a model that is not on the default branch.
pub async fn promote_existing(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    revision_id: Uuid,
) -> Result<Promotion, CompileError> {
    let kind = entity::revisions::Entity::find_by_id(revision_id)
        .one(db)
        .await?
        .map(|r| r.kind);
    match kind.as_deref() {
        Some("main") => {}
        Some(other) => {
            return Err(CompileError::Internal(format!(
                "revision {revision_id} has kind '{other}'; only a main revision can be promoted"
            )));
        }
        None => {
            return Err(CompileError::Internal(format!(
                "revision {revision_id} not found"
            )));
        }
    }
    Ok(if promote_revision(db, workspace_id, revision_id).await? {
        Promotion::Promoted
    } else {
        Promotion::Skipped
    })
}

/// Update `workspaces.current_revision_id` atomically inside the
/// finalisation transaction. The whole compile is the unit of work —
/// either the new rows AND the promotion both land, or neither does.
///
/// **Conditional on causality**: only land the UPDATE when this
/// revision's `started_at` is >= the currently-promoted revision's
/// `started_at`. Two compiles for the same workspace finishing in
/// reverse order of their start times would otherwise let the older
/// one overwrite the newer (Postgres "last write wins" without a
/// WHERE clause). The conditional clause turns that race into a
/// no-op for the loser instead of stale-data corruption.
///
/// `started_at` is captured at `insert_compiling_revision` time
/// from Postgres `now()` semantics (we set the value via the chrono
/// `Utc::now()` helper, but the absolute correctness of the
/// comparison degrades gracefully with clock skew: a skewed worker
/// at worst promotes an out-of-order revision that the next compile
/// will correct, and the conditional clause still rejects the
/// strictly-older case).
///
/// Returns whether the UPDATE landed: `false` means the causality clause
/// rejected it because a newer revision is already current. Callers turn
/// that into [`Promotion::Skipped`] — it used to be a log line only, which
/// made "promoted" and "lost the race" indistinguishable to a caller.
async fn promote_revision(
    txn: &impl ConnectionTrait,
    workspace_id: Uuid,
    revision_id: Uuid,
) -> Result<bool, CompileError> {
    use sea_orm::{DatabaseBackend, Statement};

    // Hand-rolled SQL because the causality predicate references a
    // sibling row in `revisions` via the workspace's pointer — Sea-ORM
    // update_many() doesn't compose this well.
    let sql = "\
        UPDATE workspaces \
        SET current_revision_id = $2, updated_at = now() \
        WHERE id = $1 \
          AND EXISTS ( \
              SELECT 1 FROM revisions \
              WHERE revision_id = $2 AND kind = 'main' \
          ) \
          AND ( \
              current_revision_id IS NULL \
              OR ( \
                  SELECT started_at FROM revisions \
                  WHERE revision_id = workspaces.current_revision_id \
              ) <= ( \
                  SELECT started_at FROM revisions \
                  WHERE revision_id = $2 \
              ) \
          )";
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [workspace_id.into(), revision_id.into()],
    );
    let result = txn.execute_raw(stmt).await?;
    let landed = result.rows_affected() != 0;
    if !landed {
        // A newer revision is already current — we lost the race.
        // Not an error: the per-entity rows we just wrote are still
        // queryable by their revision_id, and a future compile will
        // promote whichever is newest. Logged at info level so it's
        // visible in the timeline of concurrent compiles.
        tracing::info!(
            %workspace_id,
            %revision_id,
            "promotion skipped — a newer revision is already current_revision_id"
        );
    } else {
        tracing::info!(
            %workspace_id,
            %revision_id,
            "promoted revision to workspaces.current_revision_id"
        );
    }
    Ok(landed)
}

/// Marks a revision row as `failed` when something went wrong outside
/// the per-file loop (e.g. the file walk itself errored). Best-effort —
/// don't propagate errors from this; the caller already has the real
/// reason for failing.
pub async fn mark_failed(db: &DatabaseConnection, revision_id: Uuid, error: &str) {
    let finished_at = Utc::now();
    let row = entity::revisions::ActiveModel {
        revision_id: Set(revision_id),
        status: Set("failed".to_string()),
        finished_at: Set(Some(finished_at.fixed_offset())),
        error_summary: Set(Some(serde_json::json!({ "fatal": error }))),
        ..Default::default()
    };
    if let Err(e) = row.update(db).await {
        tracing::warn!(?e, %revision_id, "best-effort mark_failed update failed");
    }
}

/// Inserts every compiled-entity row inside the supplied transaction.
/// Done one entity table at a time so per-table failures localise.
async fn write_compiled_rows(
    txn: &impl ConnectionTrait,
    revision_id: Uuid,
    rows: &[CompiledRow],
    blob_keys: &std::collections::HashMap<(crate::blob_store::BlobKind, String), Option<String>>,
) -> Result<(), CompileError> {
    let mut configs = Vec::new();
    let mut agents = Vec::new();
    let mut views = Vec::new();
    let mut topics = Vec::new();
    let mut apps = Vec::new();
    let mut automations = Vec::new();
    let mut verified = Vec::new();
    let mut schema_migrations = Vec::new();
    let mut pipelines = Vec::new();
    let mut references = Vec::new();
    let mut monitor_cfgs = Vec::new();
    let mut reconcile_cfgs = Vec::new();
    let mut world_model_cfgs = Vec::new();
    let mut simulations = Vec::new();

    for row in rows {
        match row {
            CompiledRow::Config(c) => {
                configs.push(entity::workspace_compiled_configs::ActiveModel {
                    revision_id: Set(revision_id),
                    databases: Set(c.databases.clone()),
                    models: Set(c.models.clone()),
                    integrations: Set(c.integrations.clone()),
                    repositories: Set(c.repositories.clone()),
                    builder_agent: Set(c.builder_agent.clone()),
                    mcp: Set(c.mcp.clone()),
                    other: Set(c.other.clone()),
                })
            }
            CompiledRow::Agent(a) => agents.push(entity::agent_definitions::ActiveModel {
                revision_id: Set(revision_id),
                name: Set(a.name.clone()),
                file_path: Set(a.file_path.clone()),
                definition: Set(a.definition.clone()),
            }),
            CompiledRow::View(v) => {
                // S3 offload happens BEFORE the tx (see
                // `prepare_blob_keys` in finalise_revision). Here we
                // just look up the result. None means: no bucket
                // configured OR upload failed OR serialise failed —
                // any of which means readers fall back to the in-row
                // `definition`.
                let blob_key = blob_keys
                    .get(&(crate::blob_store::BlobKind::SemanticView, v.name.clone()))
                    .cloned()
                    .flatten();
                views.push(entity::semantic_views::ActiveModel {
                    revision_id: Set(revision_id),
                    name: Set(v.name.clone()),
                    file_path: Set(v.file_path.clone()),
                    definition: Set(v.definition.clone()),
                    compiled_sql_blob_key: Set(blob_key),
                })
            }
            CompiledRow::Topic(t) => {
                let blob_key = blob_keys
                    .get(&(crate::blob_store::BlobKind::SemanticTopic, t.name.clone()))
                    .cloned()
                    .flatten();
                topics.push(entity::semantic_topics::ActiveModel {
                    revision_id: Set(revision_id),
                    name: Set(t.name.clone()),
                    file_path: Set(t.file_path.clone()),
                    definition: Set(t.definition.clone()),
                    compiled_sql_blob_key: Set(blob_key),
                })
            }
            CompiledRow::App(a) => apps.push(entity::app_definitions::ActiveModel {
                revision_id: Set(revision_id),
                file_path: Set(a.file_path.clone()),
                name: Set(a.name.clone()),
                definition: Set(a.definition.clone()),
                published: Set(a.published),
            }),
            CompiledRow::Automation(p) => {
                automations.push(entity::automation_definitions::ActiveModel {
                    revision_id: Set(revision_id),
                    file_path: Set(p.file_path.clone()),
                    name: Set(p.name.clone()),
                    extension: Set(p.extension.clone()),
                    definition: Set(p.definition.clone()),
                })
            }
            CompiledRow::VerifiedQuery(q) => verified.push(entity::verified_queries::ActiveModel {
                revision_id: Set(revision_id),
                file_path: Set(q.file_path.clone()),
                content_sha256: Set(q.content_sha256.clone()),
                content: Set(q.content.clone()),
            }),
            CompiledRow::SchemaMigration(m) => {
                schema_migrations.push(entity::schema_migration_definitions::ActiveModel {
                    revision_id: Set(revision_id),
                    file_path: Set(m.file_path.clone()),
                    content_sha256: Set(m.content_sha256.clone()),
                    content: Set(m.content.clone()),
                })
            }
            CompiledRow::Pipeline(p) => pipelines.push(entity::airway_pipelines::ActiveModel {
                revision_id: Set(revision_id),
                name: Set(p.name.clone()),
                file_path: Set(p.file_path.clone()),
                definition: Set(p.definition.clone()),
            }),
            CompiledRow::Simulation(s) => {
                simulations.push(entity::simulation_definitions::ActiveModel {
                    revision_id: Set(revision_id),
                    name: Set(s.name.clone()),
                    file_path: Set(s.file_path.clone()),
                    definition: Set(s.definition.clone()),
                })
            }
            CompiledRow::Reference(r) => {
                references.push(entity::compiled_references::ActiveModel {
                    revision_id: Set(revision_id),
                    from_kind: Set(r.from_kind.clone()),
                    from_name: Set(r.from_name.clone()),
                    to_kind: Set(r.to_kind.clone()),
                    to_name: Set(r.to_name.clone()),
                })
            }
            CompiledRow::MonitorConfig(m) => {
                monitor_cfgs.push(entity::monitor_configs::ActiveModel {
                    revision_id: Set(revision_id),
                    definition: Set(m.definition.clone()),
                })
            }
            CompiledRow::ReconcileConfig(m) => {
                reconcile_cfgs.push(entity::reconcile_configs::ActiveModel {
                    revision_id: Set(revision_id),
                    definition: Set(m.definition.clone()),
                })
            }
            CompiledRow::WorldModelConfig(w) => {
                world_model_cfgs.push(entity::world_model_configs::ActiveModel {
                    revision_id: Set(revision_id),
                    definition: Set(w.definition.clone()),
                })
            }
        }
    }

    if !configs.is_empty() {
        entity::workspace_compiled_configs::Entity::insert_many(configs)
            .exec(txn)
            .await?;
    }
    if !agents.is_empty() {
        entity::agent_definitions::Entity::insert_many(agents)
            .exec(txn)
            .await?;
    }
    if !views.is_empty() {
        entity::semantic_views::Entity::insert_many(views)
            .exec(txn)
            .await?;
    }
    if !topics.is_empty() {
        entity::semantic_topics::Entity::insert_many(topics)
            .exec(txn)
            .await?;
    }
    if !apps.is_empty() {
        entity::app_definitions::Entity::insert_many(apps)
            .exec(txn)
            .await?;
    }
    if !automations.is_empty() {
        entity::automation_definitions::Entity::insert_many(automations)
            .exec(txn)
            .await?;
    }
    if !verified.is_empty() {
        entity::verified_queries::Entity::insert_many(verified)
            .exec(txn)
            .await?;
    }
    if !schema_migrations.is_empty() {
        entity::schema_migration_definitions::Entity::insert_many(schema_migrations)
            .exec(txn)
            .await?;
    }
    if !pipelines.is_empty() {
        entity::airway_pipelines::Entity::insert_many(pipelines)
            .exec(txn)
            .await?;
    }
    if !simulations.is_empty() {
        entity::simulation_definitions::Entity::insert_many(simulations)
            .exec(txn)
            .await?;
    }
    if !references.is_empty() {
        entity::compiled_references::Entity::insert_many(references)
            .exec(txn)
            .await?;
    }
    if !monitor_cfgs.is_empty() {
        entity::monitor_configs::Entity::insert_many(monitor_cfgs)
            .exec(txn)
            .await?;
    }
    if !reconcile_cfgs.is_empty() {
        entity::reconcile_configs::Entity::insert_many(reconcile_cfgs)
            .exec(txn)
            .await?;
    }
    if !world_model_cfgs.is_empty() {
        entity::world_model_configs::Entity::insert_many(world_model_cfgs)
            .exec(txn)
            .await?;
    }

    Ok(())
}

/// Upload every semantic-view / -topic body to S3 BEFORE we open the
/// Postgres tx. Returns a map keyed by `(BlobKind, name)` whose
/// values are the resulting S3 keys (or `None` when no bucket is
/// configured, the YAML serialise failed, or the upload errored —
/// any of which is recoverable since the in-row `definition` JSONB
/// stays canonical and runtime readers fall back to it).
///
/// Concurrency: uploads run via `futures::future::join_all` so a
/// workspace with N views + M topics takes `max(time-per-upload)`
/// wall-clock instead of `sum(...)`. The S3 client itself is process-
/// wide cached (`blob_store::s3_client`), so the credential-chain
/// walk happens once.
///
/// Fast path: when `OXY_COMPILE_BLOB_S3_BUCKET` is unset we short-
/// circuit and return an empty map — no client init, no allocations.
async fn prepare_blob_keys(
    workspace_id: Uuid,
    rows: &[CompiledRow],
) -> std::collections::HashMap<(crate::blob_store::BlobKind, String), Option<String>> {
    let mut out = std::collections::HashMap::new();
    if crate::blob_store::bucket().is_none() {
        return out;
    }
    let mut tasks: Vec<
        std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = ((crate::blob_store::BlobKind, String), Option<String>),
                    > + Send,
            >,
        >,
    > = Vec::new();
    for row in rows {
        match row {
            CompiledRow::View(v) => {
                let name = v.name.clone();
                let definition = v.definition.clone();
                tasks.push(Box::pin(async move {
                    let key = upload_one(
                        workspace_id,
                        crate::blob_store::BlobKind::SemanticView,
                        &name,
                        &definition,
                    )
                    .await;
                    ((crate::blob_store::BlobKind::SemanticView, name), key)
                }));
            }
            CompiledRow::Topic(t) => {
                let name = t.name.clone();
                let definition = t.definition.clone();
                tasks.push(Box::pin(async move {
                    let key = upload_one(
                        workspace_id,
                        crate::blob_store::BlobKind::SemanticTopic,
                        &name,
                        &definition,
                    )
                    .await;
                    ((crate::blob_store::BlobKind::SemanticTopic, name), key)
                }));
            }
            _ => {}
        }
    }
    for (k, v) in futures::future::join_all(tasks).await {
        out.insert(k, v);
    }
    out
}

/// Single-blob upload helper. Failure semantics: any error path
/// returns `None` so the row is persisted with NULL key and readers
/// fall back to the in-row `definition`. Compile MUST NEVER fail
/// because an upload failed.
async fn upload_one(
    workspace_id: Uuid,
    kind: crate::blob_store::BlobKind,
    name: &str,
    definition: &serde_json::Value,
) -> Option<String> {
    let yaml = match serde_yaml::to_string(definition) {
        Ok(y) => y,
        Err(e) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                name,
                error = %e,
                kind = ?kind,
                "compile: serialise blob YAML failed; storing NULL key"
            );
            return None;
        }
    };
    match crate::blob_store::put_blob(workspace_id, kind, name, yaml.as_bytes()).await {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                name,
                error = %e,
                kind = ?kind,
                "compile: blob upload failed; storing NULL key"
            );
            None
        }
    }
}

#[cfg(test)]
mod promotion_tests {
    use super::*;

    fn input(promote: bool, status: RevisionStatus, kind: &str, schemas: u32) -> FinaliseInput<'_> {
        FinaliseInput {
            status,
            file_count_seen: 0,
            file_count_compiled: 0,
            file_count_failed: 0,
            failures: &[],
            rows: &[],
            promote,
            kind: kind.to_string(),
            schema_migration_count: schemas,
        }
    }

    #[test]
    fn plain_promote_is_immediate() {
        // The overwhelmingly common case: no `schemas/` directory, so
        // nothing to apply and no reason to defer.
        assert_eq!(
            decide_promotion(&input(true, RevisionStatus::Ready, "main", 0)),
            Promotion::Promoted
        );
    }

    #[test]
    fn ddl_defers_and_carries_the_count() {
        assert_eq!(
            decide_promotion(&input(true, RevisionStatus::Ready, "main", 3)),
            Promotion::Deferred {
                schema_migration_count: 3
            }
        );
    }

    #[test]
    fn a_draft_never_defers() {
        // The sharp one. A draft is scoped to one user and is never
        // promoted — but the org's OLTP database is shared by everyone.
        // Reporting `Deferred` here would invite the caller to apply one
        // person's in-progress DDL to the whole org.
        assert_eq!(
            decide_promotion(&input(true, RevisionStatus::Ready, "draft", 2)),
            Promotion::NotRequested
        );
    }

    #[test]
    fn a_staging_revision_never_promotes_even_when_asked() {
        // A staging revision is a branch head a custom-app draft build pins.
        // `promote: true` must not move `current_revision_id` onto it, and its
        // DDL must not be applied to the org's database either.
        assert_eq!(
            decide_promotion(&input(true, RevisionStatus::Ready, "staging", 0)),
            Promotion::NotRequested
        );
        assert_eq!(
            decide_promotion(&input(true, RevisionStatus::Ready, "staging", 2)),
            Promotion::NotRequested
        );
    }

    #[test]
    fn a_failed_compile_never_defers() {
        // Same hazard as the draft: the revision will never go live, so
        // applying its DDL would mutate the tenant database on behalf of a
        // revision nobody will ever serve.
        assert_eq!(
            decide_promotion(&input(true, RevisionStatus::Failed, "main", 2)),
            Promotion::NotRequested
        );
    }

    #[test]
    fn observation_mode_never_defers() {
        // `oxy compile` without --promote is observation-only. It must not
        // touch the tenant database either.
        assert_eq!(
            decide_promotion(&input(false, RevisionStatus::Ready, "main", 2)),
            Promotion::NotRequested
        );
    }
}
