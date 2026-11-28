//! `POST /admin/orgs/{org_id}/oltp/apply` — apply a workspace's compiled
//! `schemas/*.sql` to the org's database, for staff.
//!
//! This was the Rust `oxy oltp apply` (deleted 2026-11-24), which ran on an
//! operator's laptop with the deployment's database URL and sealing key. Like
//! [`super::confinement`], there is no router here: the route is declared by
//! `crate::admin::oltp`, which fences it by scope and writes an
//! audit row before delegating to [`apply`] (`oltp.migrations.apply_started`)
//! and one after (`oltp.migrations.applied`).
//!
//! **Why it still exists.** A compile applies a revision's DDL before it
//! promotes it (`compile-runtime`'s apply-then-promote), so the ordinary path
//! needs no operator. Two cases do: an org whose database was provisioned
//! AFTER its workspace compiled (the compile promoted without DDL, because
//! there was nowhere to apply it), and grants that drifted —
//! [`migrator::apply_to_org`] re-grants every writer over its schema on every
//! run, applied files or not, so this is also the repair for
//! `permission denied for table`.
//!
//! **It reads the compiled revision, never a filesystem.** The migrations are
//! `schema_migration_definitions` rows keyed by `revision_id`; the revision is
//! the workspace's newest `ready` one, as the Rust command chose it — not
//! `current_revision_id`, because a revision whose DDL failed is deferred and
//! unpromoted, and applying it is exactly what an operator is here to do.
//!
//! **A migration that ran and failed answers 200**, with `ok: false` and the
//! file: the apply ran, and its answer is the verdict (`oxyc oltp apply` turns
//! it into exit 9), the shape `…/oltp/audit` and `looker test` use. Only the
//! cases where nothing could run are error statuses: no database (404), an
//! ambiguous workspace (400), no successful compile (409), another apply
//! holding the tenant's lock (503, retry), a database that would not open a
//! session (502), and a budget spent before any DDL ran (504, re-run). An
//! apply [`APPLY_BUDGET`] stops part way is a 200 with `ok: false` and
//! `timed_out: true`, naming the files that committed before the cut.

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use tracing::{error, instrument, warn};
use uuid::Uuid;

use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_platform::db::establish_connection;

use oxy_oltp::entity::tenants::Model as TenantRow;
use oxy_oltp::migrator::{self, MigrateError, MigrateOutcome};

/// How long one call may apply for. Under the router's 60-second global
/// timeout (`finalize_router`), so the handler answers — and the shim audits —
/// before the layer drops it. A cut-off apply is safe to re-run: each file
/// commits with its ledger row, so the next call resumes at the first file
/// the database has not run.
pub const APPLY_BUDGET: std::time::Duration = std::time::Duration::from_secs(50);

/// The body. Every field is optional, so `{}` applies the org's only
/// workspace — and an out-of-scope caller meets the fence's 404, not a 422.
#[derive(Debug, Default, Deserialize)]
pub struct ApplyRequest {
    /// Which workspace claims the schema namespaces and supplies the
    /// migrations. Required when the org has more than one.
    #[serde(default)]
    pub workspace_id: Option<Uuid>,
}

/// The migration that stopped the apply. Files before it stay applied and
/// recorded; nothing after it ran.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FailedMigration {
    pub file_path: String,
    pub message: String,
}

/// What an apply did. `ok` is the verdict a script gates on.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ApplyResponse {
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub revision_id: Uuid,
    /// Files this call applied, in order — on a failure too, since each file
    /// commits on its own. Empty when the store was up to date, or when the
    /// first pending file failed.
    pub applied: Vec<String>,
    /// Files the store had already run, checksums matching.
    pub already_applied: usize,
    pub failed: Option<FailedMigration>,
    /// The apply ran past [`APPLY_BUDGET`] and was stopped: `applied` is what
    /// landed, and a re-run continues from there.
    pub timed_out: bool,
    pub ok: bool,
}

impl ApplyResponse {
    fn new(org_id: Uuid, workspace_id: Uuid, revision_id: Uuid) -> Self {
        Self {
            org_id,
            workspace_id,
            revision_id,
            applied: Vec::new(),
            already_applied: 0,
            failed: None,
            timed_out: false,
            ok: true,
        }
    }
}

/// Why nothing could be applied.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("org {0} has no workspace")]
    NoWorkspace(Uuid),
    #[error("workspace {workspace_id} is not in org {org_id}")]
    WorkspaceNotInOrg { workspace_id: Uuid, org_id: Uuid },
    #[error(
        "org {org_id} has {count} workspaces; pass --workspace to say which one \
         claims the schema namespaces and supplies the migrations:\n{listed}"
    )]
    AmbiguousWorkspace {
        org_id: Uuid,
        count: usize,
        listed: String,
    },
    #[error("workspace {0} has no successful compile — push to its default branch first")]
    NoReadyRevision(Uuid),
    #[error("the apply ran past {0}s before any migration ran, so nothing was applied — re-run it")]
    TimedOut(u64),
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error(transparent)]
    Migrate(#[from] MigrateError),
}

/// The status each refusal deserves. A failed or edited migration is not here:
/// it ran, so it is a verdict in the body ([`verdict`]), not a status.
pub(crate) fn apply_status(e: ApplyError) -> (StatusCode, String) {
    let code = match &e {
        ApplyError::NoWorkspace(_)
        | ApplyError::WorkspaceNotInOrg { .. }
        | ApplyError::Migrate(MigrateError::NotProvisioned(_)) => StatusCode::NOT_FOUND,
        ApplyError::AmbiguousWorkspace { .. } => StatusCode::BAD_REQUEST,
        ApplyError::NoReadyRevision(_) => StatusCode::CONFLICT,
        // Someone else is mid-apply: everything is fine, try again shortly.
        ApplyError::Migrate(MigrateError::Locked { .. }) => StatusCode::SERVICE_UNAVAILABLE,
        // The tenant database refused or could not be reached: nothing ran,
        // which is not the same answer as "it ran and failed".
        ApplyError::Migrate(MigrateError::Connect(_)) => StatusCode::BAD_GATEWAY,
        // Cut off by [`APPLY_BUDGET`]; retryable, and a re-run resumes.
        ApplyError::TimedOut(_) => StatusCode::GATEWAY_TIMEOUT,
        ApplyError::Db(_) | ApplyError::Migrate(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    if code == StatusCode::INTERNAL_SERVER_ERROR {
        error!("oltp apply: {e}");
    }
    (code, e.to_string())
}

/// Fold the migrator's answer into the response: a failed or edited migration
/// is a verdict (`ok: false`); anything else that went wrong is an error.
///
/// `outcome` is what ran before any failure: files this call committed stay
/// committed, so the body (and the audit row built from it) names them.
pub(crate) fn verdict(
    mut response: ApplyResponse,
    outcome: MigrateOutcome,
    result: Result<(), MigrateError>,
) -> Result<ApplyResponse, ApplyError> {
    response.applied = outcome.applied;
    response.already_applied = outcome.already_applied;
    match result {
        Ok(()) => {}
        Err(MigrateError::Failed { file_path, message }) => {
            response.failed = Some(FailedMigration { file_path, message });
            response.ok = false;
        }
        Err(MigrateError::ChecksumMismatch { file_path }) => {
            // The migrator's own sentence, which says what to do instead.
            let message = MigrateError::ChecksumMismatch {
                file_path: file_path.clone(),
            }
            .to_string();
            response.failed = Some(FailedMigration { file_path, message });
            response.ok = false;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(response)
}

/// Apply every compiled migration the org's database has not run, then
/// re-grant every writer over its schema.
#[instrument(skip(user, body), fields(user_id = %user.id, org_id = %org_id))]
pub async fn apply(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<ApplyRequest>,
) -> Result<Json<ApplyResponse>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Answer before the router's 60-second global timeout does. That layer
    // drops the handler with a bare 408, which the shim could not audit and
    // `oxyc` would read as a refused request; this says what happened.
    let deadline = tokio::time::Instant::now() + APPLY_BUDGET;
    run(&db, org_id, body.workspace_id, user.label(), deadline)
        .await
        .map(Json)
        .map_err(apply_status)
}

/// The Rust command's order: the tenant first (no database is the most common
/// answer, and the cheapest), then the workspace, then its newest ready
/// revision, then the owner credential.
///
/// Both halves share one `deadline`. Running out of it before any DDL is a
/// 504 that applied nothing; running out during the apply is a verdict that
/// names the files that committed, because `outcome` lives here, outside the
/// future the timeout drops.
async fn run(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace: Option<Uuid>,
    who: &str,
    deadline: tokio::time::Instant,
) -> Result<ApplyResponse, ApplyError> {
    let timed_out = || ApplyError::TimedOut(APPLY_BUDGET.as_secs());
    let (tenant, workspace, revision_id) =
        tokio::time::timeout_at(deadline, resolve(db, org_id, workspace))
            .await
            .map_err(|_| timed_out())??;
    let dsn = migrator::owner_dsn(&tenant)?;

    warn!(user = %who, org_id = %org_id, workspace_id = %workspace.id, revision_id = %revision_id,
        "applying an org's OLTP schema migrations");
    let target = migrator::ApplyTarget {
        org_id,
        revision_id,
        owner_dsn: &dsn,
        tenant_row_id: tenant.id,
        owner_role: &tenant.owner_role,
    };
    let mut outcome = MigrateOutcome::default();
    let applied =
        tokio::time::timeout_at(deadline, migrator::apply_into(db, &target, &mut outcome)).await;
    let response = ApplyResponse::new(org_id, workspace.id, revision_id);
    match applied {
        Ok(result) => verdict(response, outcome, result),
        Err(_) => Ok(cut_off(response, outcome)),
    }
}

/// The tenant, the workspace and its newest ready revision — everything read
/// before any DDL runs.
async fn resolve(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace: Option<Uuid>,
) -> Result<(TenantRow, entity::workspaces::Model, Uuid), ApplyError> {
    let tenant = migrator::tenant_for_org(db, org_id).await?;
    let workspace = resolve_workspace(db, org_id, workspace).await?;
    let revision_id = latest_ready_revision(db, workspace.id).await?;
    Ok((tenant, workspace, revision_id))
}

/// An apply [`APPLY_BUDGET`] stopped part way. Not an error status: files it
/// committed stay on the customer's database, so the body names them
/// (`applied`), says it is not done (`ok: false`, `timed_out: true`), and the
/// shim's audit row records the same. A re-run resumes at the first file the
/// database has not run; the file in flight rolled back with its transaction.
pub(crate) fn cut_off(mut response: ApplyResponse, outcome: MigrateOutcome) -> ApplyResponse {
    response.applied = outcome.applied;
    response.already_applied = outcome.already_applied;
    response.ok = false;
    response.timed_out = true;
    response
}

/// The org's workspace, refusing to guess when there is more than one.
///
/// For a multi-workspace org, "whichever row came back first" would decide
/// which workspace CLAIMS each schema namespace and whose `schemas/*.sql`
/// reach the tenant — a re-run could stamp the claim on a different
/// workspace, or trip `SchemaNamespaceClaimed` against itself. Name it.
async fn resolve_workspace(
    db: &DatabaseConnection,
    org_id: Uuid,
    requested: Option<Uuid>,
) -> Result<entity::workspaces::Model, ApplyError> {
    let all = entity::prelude::Workspaces::find()
        .filter(entity::workspaces::Column::OrgId.eq(Some(org_id)))
        .order_by_asc(entity::workspaces::Column::Id)
        .all(db)
        .await?;
    pick_workspace(org_id, all, requested)
}

fn pick_workspace(
    org_id: Uuid,
    all: Vec<entity::workspaces::Model>,
    requested: Option<Uuid>,
) -> Result<entity::workspaces::Model, ApplyError> {
    if let Some(id) = requested {
        return all
            .into_iter()
            .find(|w| w.id == id)
            .ok_or(ApplyError::WorkspaceNotInOrg {
                workspace_id: id,
                org_id,
            });
    }
    match all.len() {
        0 => Err(ApplyError::NoWorkspace(org_id)),
        1 => Ok(all.into_iter().next().expect("length checked")),
        count => Err(ApplyError::AmbiguousWorkspace {
            org_id,
            count,
            listed: all
                .iter()
                .map(|w| format!("  {}  {}", w.id, w.name))
                .collect::<Vec<_>>()
                .join("\n"),
        }),
    }
}

/// Newest **ready** revision of the workspace. A failed compile must never
/// reach a customer's database.
async fn latest_ready_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Uuid, ApplyError> {
    entity::prelude::Revisions::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
        .filter(entity::revisions::Column::Status.eq("ready"))
        .order_by_desc(entity::revisions::Column::StartedAt)
        .one(db)
        .await?
        .map(|r| r.revision_id)
        .ok_or(ApplyError::NoReadyRevision(workspace_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ApplyResponse {
        ApplyResponse::new(Uuid::nil(), Uuid::nil(), Uuid::nil())
    }

    /// The statuses `oxyc oltp apply` maps onto its exit codes: 404 → 5,
    /// 400 / 409 → 6, 502 / 503 → 7 (retryable).
    #[test]
    fn each_refusal_has_the_status_its_exit_code_needs() {
        let org = Uuid::new_v4();
        let cases = [
            (
                ApplyError::Migrate(MigrateError::NotProvisioned(org)),
                StatusCode::NOT_FOUND,
            ),
            (ApplyError::NoWorkspace(org), StatusCode::NOT_FOUND),
            (
                ApplyError::WorkspaceNotInOrg {
                    workspace_id: org,
                    org_id: org,
                },
                StatusCode::NOT_FOUND,
            ),
            (
                ApplyError::AmbiguousWorkspace {
                    org_id: org,
                    count: 2,
                    listed: String::new(),
                },
                StatusCode::BAD_REQUEST,
            ),
            (ApplyError::NoReadyRevision(org), StatusCode::CONFLICT),
            (
                ApplyError::Migrate(MigrateError::Locked { org_id: org }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ApplyError::Migrate(MigrateError::Connect("refused".into())),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ApplyError::Migrate(MigrateError::Owner("no password".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (e, want) in cases {
            let label = e.to_string();
            assert_eq!(apply_status(e).0, want, "{label}");
        }
    }

    /// A migration that ran and failed is the verdict, not an error status:
    /// `ok: false`, the file, and Postgres' reason.
    #[test]
    fn a_failed_migration_is_a_verdict_in_the_body() {
        let out = verdict(
            base(),
            MigrateOutcome::default(),
            Err(MigrateError::Failed {
                file_path: "schemas/0002_orders.sql".into(),
                message: "syntax error".into(),
            }),
        )
        .expect("a failed migration is a 200");
        assert!(!out.ok);
        assert_eq!(
            out.failed,
            Some(FailedMigration {
                file_path: "schemas/0002_orders.sql".into(),
                message: "syntax error".into(),
            })
        );
    }

    /// An edited shipped migration is refused the same way, and keeps the
    /// migrator's message, which says what to do instead.
    #[test]
    fn an_edited_migration_is_a_verdict_that_names_the_fix() {
        let out = verdict(
            base(),
            MigrateOutcome::default(),
            Err(MigrateError::ChecksumMismatch {
                file_path: "schemas/0001_init.sql".into(),
            }),
        )
        .expect("a checksum mismatch is a 200");
        assert!(!out.ok);
        let failed = out.failed.expect("names the file");
        assert_eq!(failed.file_path, "schemas/0001_init.sql");
        assert!(
            failed.message.contains("add a new file"),
            "{}",
            failed.message
        );
    }

    #[test]
    fn a_clean_apply_reports_what_it_ran() {
        let out = verdict(
            base(),
            MigrateOutcome {
                applied: vec!["schemas/0003_x.sql".into()],
                already_applied: 2,
            },
            Ok(()),
        )
        .expect("ok");
        assert!(out.ok);
        assert_eq!(out.applied, vec!["schemas/0003_x.sql".to_string()]);
        assert_eq!(out.already_applied, 2);
        assert_eq!(out.failed, None);
    }

    #[test]
    fn the_lock_and_the_connection_stay_errors_not_verdicts() {
        assert!(matches!(
            verdict(
                base(),
                MigrateOutcome::default(),
                Err(MigrateError::Locked {
                    org_id: Uuid::nil()
                })
            ),
            Err(ApplyError::Migrate(MigrateError::Locked { .. }))
        ));
        assert!(matches!(
            verdict(
                base(),
                MigrateOutcome::default(),
                Err(MigrateError::Connect("x".into()))
            ),
            Err(ApplyError::Migrate(MigrateError::Connect(_)))
        ));
    }

    /// Files this call committed before the failing one are reported, not
    /// dropped: they are owner-level DDL that stays on the customer's database,
    /// and the audit row is built from this body.
    #[test]
    fn a_later_failure_keeps_the_files_that_committed_before_it() {
        let out = verdict(
            base(),
            MigrateOutcome {
                applied: vec![
                    "schemas/0001_a.sql".to_string(),
                    "schemas/0002_b.sql".to_string(),
                ],
                already_applied: 4,
            },
            Err(MigrateError::Failed {
                file_path: "schemas/0003_c.sql".into(),
                message: "boom".into(),
            }),
        )
        .expect("a failed migration is a 200");
        assert!(!out.ok);
        assert_eq!(
            out.applied,
            vec![
                "schemas/0001_a.sql".to_string(),
                "schemas/0002_b.sql".to_string()
            ]
        );
        assert_eq!(out.already_applied, 4);
        assert_eq!(
            out.failed.map(|f| f.file_path).as_deref(),
            Some("schemas/0003_c.sql")
        );
    }

    /// An apply stopped by the budget mid-way is a body, not a bare status:
    /// the files that committed before the cut are named, so the audit row
    /// and `oxyc` both list them.
    #[test]
    fn a_cut_off_apply_names_the_files_that_landed() {
        let out = cut_off(
            base(),
            MigrateOutcome {
                applied: vec![
                    "schemas/0001_a.sql".to_string(),
                    "schemas/0002_b.sql".to_string(),
                ],
                already_applied: 1,
            },
        );
        assert!(!out.ok);
        assert!(out.timed_out);
        assert_eq!(out.failed, None);
        assert_eq!(
            out.applied,
            vec![
                "schemas/0001_a.sql".to_string(),
                "schemas/0002_b.sql".to_string()
            ]
        );
        assert_eq!(out.already_applied, 1);
    }

    /// Run out of budget before any DDL (resolving the tenant, the workspace
    /// or the revision) and nothing landed: a retryable 504 saying so.
    #[test]
    fn a_cut_off_before_any_ddl_answers_before_the_router_does() {
        let (code, body) = apply_status(ApplyError::TimedOut(APPLY_BUDGET.as_secs()));
        assert_eq!(code, StatusCode::GATEWAY_TIMEOUT);
        assert!(body.contains("re-run"), "{body}");
    }

    /// The workspace choice the Rust command made: the named one if it is in
    /// the org, the only one, or a refusal that lists them.
    #[test]
    fn the_workspace_is_named_or_unambiguous() {
        let org = Uuid::new_v4();
        assert!(matches!(
            pick_workspace(org, vec![], None),
            Err(ApplyError::NoWorkspace(_))
        ));
        assert!(matches!(
            pick_workspace(org, vec![], Some(Uuid::new_v4())),
            Err(ApplyError::WorkspaceNotInOrg { .. })
        ));
    }

    #[test]
    fn an_empty_body_parses() {
        let body: ApplyRequest = serde_json::from_str("{}").expect("all optional");
        assert_eq!(body.workspace_id, None);
    }
}
