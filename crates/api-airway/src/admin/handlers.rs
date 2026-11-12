//! Handlers for `/api/admin/airway/config` — read (Task 1) and write
//! (Task 2).
//!
//! # Scope
//!
//! This surface is gated by `cap(Action::PlatformOperate)`, and that gate
//! decides on `Resource::platform()` — a resource with no org — so it cannot
//! consult the caller's **scope**. Capabilities gate verbs; scope filters rows
//! (see `admin::scope` and `platform_cap_guard`). Every per-workspace route
//! here therefore carries the row-level half itself:
//!
//! * the two override writes fence with [`deny_out_of_scope_for_workspace`]
//!   *before* touching the table;
//! * [`get_config`] filters the overrides it returns to the caller's reach.
//!
//! The third route that returns per-tenant rows is the policy preview, and it
//! lives next door: `super::preview::preview_policy` takes the same
//! [`scope_org_filter`] and hands it to the scan. This list named only the
//! routes in *this file* for one release, which read as a complete inventory of
//! the surface's fences and was not one — the preview was returning every
//! tenant's workspace ids and `.airway.yml` paths the whole time. It is
//! enumerated here because the sentence above claims "every per-workspace
//! route", and that claim has to be checkable against the router.
//!
//! **Known and accepted: the global row stays fleet-wide.** A scoped operator
//! who reaches one org can still edit the `workspace_id IS NULL` row that
//! governs every tenant's pipelines — see `admin::mod`'s note at the mount
//! point for why that was taken rather than overlooked, and what would have to
//! change to close it.

use std::collections::{HashMap, HashSet};

use agentic_airway::{AirwayAdmission, AirwayError};
use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, FixedOffset};
use entity::airway_source_config::Column as ConfigColumn;
use entity::{airway_source_config, workspaces};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::KNOWN_SOURCE_KINDS;
use oxy_app::surface::admin_db::{connect, db_err, error_body};
use oxy_app::surface::admin_scope as scope;
use oxy_app::surface::admin_scope::scope_org_filter;

/// A config row's two editable fields plus its freshness. Shared shape for
/// both the global row and a workspace override — `updated_at` lets Task 5's
/// cards show staleness without a second call.
#[derive(Serialize)]
pub struct ConfigValues {
    pub contract_policy: Option<String>,
    pub environment: Option<String>,
    pub updated_at: DateTime<FixedOffset>,
}

#[derive(Serialize)]
pub struct WorkspaceOverride {
    pub workspace_id: Uuid,
    /// `None` when the workspace row is gone — the `LEFT JOIN` still
    /// surfaces the override, just without a display name.
    pub workspace_name: Option<String>,
    pub values: ConfigValues,
}

#[derive(Serialize)]
pub struct SourceKindConfig {
    pub source_kind: String,
    /// The `workspace_id IS NULL` row, if one exists.
    pub global: Option<ConfigValues>,
    pub overrides: Vec<WorkspaceOverride>,
}

#[derive(Serialize)]
pub struct AirwayConfigResponse {
    pub kinds: Vec<SourceKindConfig>,
}

/// `GET /api/admin/airway/config`. Pure Postgres read (`FleetOk` in
/// `role_manifest.rs`) — grouped per [`KNOWN_SOURCE_KINDS`], each with its
/// global row (if any) and its per-workspace overrides. A kind with no row
/// at all still appears (`global: None`) so the admin page can create the
/// first row for it.
///
/// **Overrides are filtered to the caller's platform scope.** A bounded grant
/// sees only the overrides belonging to workspaces in the orgs it reaches; an
/// unbounded grant (and the Global Owner) sees all of them, exactly as before.
/// Without this, a two-org operator could read — and, through the picker, name
/// — every other tenant's workspace.
///
/// The **global row is not filtered**: it is one fleet-wide row per kind, not a
/// tenant's row, and it is what every override is defined against. Hiding it
/// would leave a scoped operator editing overrides whose inherited fields they
/// cannot see. That the same operator can *write* it is the accepted residual
/// recorded at the mount point in `admin::mod`.
pub async fn get_config(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<AirwayConfigResponse>, Response> {
    let db = connect().await?;
    // The LENIENT read-path filter (`Err` → don't filter), deliberately: the
    // same one `admin::oxy_access` reuses, and the same split `apps::handlers`
    // documents — a read prefers showing rows to presenting an empty console as
    // though it were the truth, while the writes below fail closed. One rule,
    // stated once, in the module that owns it.
    let scope = scope_org_filter(&db, &actor).await;
    let resp = list_airway_config(&db, scope.as_deref())
        .await
        .map_err(db_err)?;
    Ok(Json(resp))
}

/// One query — `find_also_related` issues a single `LEFT JOIN` against
/// `workspaces`, so a deleted workspace's override still reports (with
/// `workspace_name: None`) instead of vanishing. Grouped in memory by
/// `source_kind`, partitioned on `workspace_id.is_none()`; never one query
/// per kind.
///
/// Reports each row's raw stored value, not the merged one —
/// `agentic_pipeline::airway_config::resolve_admission` owns merging a
/// sparse override onto the global row. An override that sets only
/// `contract_policy` reports `environment: None` here even though a caller
/// resolving admission for that workspace would inherit the global row's
/// `environment`.
///
/// `scope_orgs` is the caller's reach: `None` = unbounded (a Global Owner, or
/// a `scope_all` grant), `Some(orgs)` = a bounded grant, which sees only the
/// overrides whose workspace belongs to one of those orgs. `Some(&[])` is a
/// real answer — a grant bounded to nothing — and correctly yields no
/// overrides at all. The same join that supplies each override's display name
/// supplies the org to fence on, so this costs no extra query.
///
/// A workspace with a NULL `org_id`, or an override whose workspace row the
/// join could not resolve, is **dropped for a bounded grant** — a null org is
/// by definition not in `Scope::Orgs(..)`. Same direction as
/// `scope::deny_out_of_scope_opt`, whose `None` arm exists because the obvious
/// `if let Some(org)` spelling is not a check that passes but no check at all.
pub(crate) async fn list_airway_config(
    db: &DatabaseConnection,
    scope_orgs: Option<&[Uuid]>,
) -> Result<AirwayConfigResponse, DbErr> {
    let rows = airway_source_config::Entity::find()
        .find_also_related(workspaces::Entity)
        .all(db)
        .await?;

    // Hashed once, not re-scanned per row: this is a cross-tenant listing, so
    // the row count is the fleet's overrides and the grant can hold every org a
    // partner distributes to.
    let allowed: Option<HashSet<Uuid>> = scope_orgs.map(|orgs| orgs.iter().copied().collect());

    let mut global: HashMap<String, ConfigValues> = HashMap::new();
    let mut overrides: HashMap<String, Vec<WorkspaceOverride>> = HashMap::new();
    for (row, ws) in rows {
        let values = ConfigValues {
            contract_policy: row.contract_policy,
            environment: row.environment,
            updated_at: row.updated_at,
        };
        match row.workspace_id {
            None => {
                global.insert(row.source_kind, values);
            }
            Some(workspace_id) => {
                if let Some(allowed) = &allowed {
                    let reaches = ws
                        .as_ref()
                        .and_then(|w| w.org_id)
                        .is_some_and(|org_id| allowed.contains(&org_id));
                    if !reaches {
                        continue;
                    }
                }
                overrides
                    .entry(row.source_kind)
                    .or_default()
                    .push(WorkspaceOverride {
                        workspace_id,
                        workspace_name: ws.map(|w| w.name),
                        values,
                    });
            }
        }
    }

    let kinds = KNOWN_SOURCE_KINDS
        .iter()
        .map(|&kind| SourceKindConfig {
            source_kind: kind.to_string(),
            global: global.remove(kind),
            overrides: overrides.remove(kind).unwrap_or_default(),
        })
        .collect();

    Ok(AirwayConfigResponse { kinds })
}

// Writes (Task 2)

/// Body for both `PUT` routes. `None` on either field clears it back to
/// "inherit" — this is a replace, not a patch: a caller that wants to keep
/// the existing `environment` while changing `contract_policy` must send the
/// current `environment` back too.
#[derive(Deserialize)]
pub struct UpsertConfigRequest {
    /// `None` clears the field back to "inherit".
    pub contract_policy: Option<String>,
    pub environment: Option<String>,
}

/// Everything that can go wrong writing a config row. The two validation
/// variants are operator mistakes (400); [`ConfigWriteError::Db`] is ours
/// (500, via [`db_err`]).
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigWriteError {
    /// A `source_kind` outside [`KNOWN_SOURCE_KINDS`] — writing one anyway
    /// would produce a row [`list_airway_config`] can never surface again,
    /// since that groups strictly by the known list.
    #[error("unknown airway source kind `{0}` (expected one of {KNOWN_SOURCE_KINDS:?})")]
    UnknownSourceKind(String),
    /// A `contract_policy` / `environment` spelling `AirwayAdmission` doesn't
    /// recognize. Its `Display` already names the accepted spellings.
    #[error(transparent)]
    Admission(#[from] AirwayError),
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Maps to the same `{ code, message }` body every other admin write route
/// returns (`internal_jobs::ErrorBody`/`error_body`) — an airway-config `400`
/// must not be the one shape the frontend has to special-case.
fn config_write_err(err: ConfigWriteError) -> Response {
    match err {
        ConfigWriteError::Db(e) => db_err(e),
        ConfigWriteError::UnknownSourceKind(_) => error_body(
            StatusCode::BAD_REQUEST,
            "unknown_source_kind",
            Some(err.to_string()),
        ),
        ConfigWriteError::Admission(_) => error_body(
            StatusCode::BAD_REQUEST,
            "invalid_admission_policy",
            Some(err.to_string()),
        ),
    }
}

/// Shared validate-then-upsert body for both `PUT` routes. `workspace_id:
/// None` targets the global row; `Some(id)` targets that workspace's
/// override. Validates `source_kind` against [`KNOWN_SOURCE_KINDS`] and the
/// two policy strings against [`AirwayAdmission::from_strings`] *before*
/// touching the database — a typo must never reach the table.
///
/// Upserts on the partial unique index stage 2 created for this
/// `workspace_id`-nullness case: `airway_source_config_global_uniq` (on
/// `source_kind` `WHERE workspace_id IS NULL`) for the global row, or
/// `airway_source_config_workspace_uniq` (on `source_kind, workspace_id`
/// `WHERE workspace_id IS NOT NULL`) for an override. Postgres treats NULLs
/// as distinct, so the `target_and_where` predicate must match the index's
/// `WHERE` clause exactly or Postgres can't infer which partial index the
/// conflict targets.
async fn upsert_config(
    db: &DatabaseConnection,
    source_kind: &str,
    workspace_id: Option<Uuid>,
    contract_policy: Option<&str>,
    environment: Option<&str>,
) -> Result<(), ConfigWriteError> {
    if !KNOWN_SOURCE_KINDS.contains(&source_kind) {
        return Err(ConfigWriteError::UnknownSourceKind(source_kind.to_string()));
    }
    AirwayAdmission::from_strings(contract_policy, environment)?;

    let model = airway_source_config::ActiveModel {
        source_kind: Set(source_kind.to_string()),
        workspace_id: Set(workspace_id),
        contract_policy: Set(contract_policy.map(str::to_string)),
        environment: Set(environment.map(str::to_string)),
        updated_at: Set(chrono::Utc::now().fixed_offset()),
        ..Default::default()
    };

    let on_conflict = match workspace_id {
        None => OnConflict::column(ConfigColumn::SourceKind)
            .update_columns([
                ConfigColumn::ContractPolicy,
                ConfigColumn::Environment,
                ConfigColumn::UpdatedAt,
            ])
            .target_and_where(ConfigColumn::WorkspaceId.is_null())
            .to_owned(),
        Some(_) => OnConflict::columns([ConfigColumn::SourceKind, ConfigColumn::WorkspaceId])
            .update_columns([
                ConfigColumn::ContractPolicy,
                ConfigColumn::Environment,
                ConfigColumn::UpdatedAt,
            ])
            .target_and_where(ConfigColumn::WorkspaceId.is_not_null())
            .to_owned(),
    };

    airway_source_config::Entity::insert(model)
        .on_conflict(on_conflict)
        .exec(db)
        .await?;
    Ok(())
}

/// Create or replace the global (`workspace_id IS NULL`) row for
/// `source_kind`.
pub(crate) async fn upsert_global(
    db: &DatabaseConnection,
    source_kind: &str,
    contract_policy: Option<&str>,
    environment: Option<&str>,
) -> Result<(), ConfigWriteError> {
    upsert_config(db, source_kind, None, contract_policy, environment).await
}

/// Create or replace the `workspace_id`-scoped override row for
/// `source_kind`. Leaves the global row (if any) untouched.
pub(crate) async fn upsert_override(
    db: &DatabaseConnection,
    source_kind: &str,
    workspace_id: Uuid,
    contract_policy: Option<&str>,
    environment: Option<&str>,
) -> Result<(), ConfigWriteError> {
    upsert_config(
        db,
        source_kind,
        Some(workspace_id),
        contract_policy,
        environment,
    )
    .await
}

/// Delete the global row for `source_kind`, if one exists. Deleting a row
/// that isn't there is a no-op, not an error — `DELETE` is idempotent. Any
/// per-workspace overrides for this kind are untouched: they simply lose the
/// global row they were inheriting unset fields from.
///
/// Validates `source_kind` for the same reason [`upsert_config`] does, even
/// though a typo could only ever delete nothing: an idempotent no-op and a
/// misspelled kind are indistinguishable at the wire, so without this the
/// operator reads a `204` and a success toast for a delete that was never
/// going to match. Idempotence is about the row, not about the vocabulary.
pub(crate) async fn delete_global(
    db: &DatabaseConnection,
    source_kind: &str,
) -> Result<(), ConfigWriteError> {
    if !KNOWN_SOURCE_KINDS.contains(&source_kind) {
        return Err(ConfigWriteError::UnknownSourceKind(source_kind.to_string()));
    }
    airway_source_config::Entity::delete_many()
        .filter(ConfigColumn::SourceKind.eq(source_kind))
        .filter(ConfigColumn::WorkspaceId.is_null())
        .exec(db)
        .await?;
    Ok(())
}

/// Delete the `workspace_id`-scoped override for `source_kind`, if one
/// exists. Leaves the global row untouched — the workspace simply goes back
/// to inheriting it in full. Same `source_kind` validation as
/// [`delete_global`], for the same reason.
pub(crate) async fn delete_override(
    db: &DatabaseConnection,
    source_kind: &str,
    workspace_id: Uuid,
) -> Result<(), ConfigWriteError> {
    if !KNOWN_SOURCE_KINDS.contains(&source_kind) {
        return Err(ConfigWriteError::UnknownSourceKind(source_kind.to_string()));
    }
    airway_source_config::Entity::delete_many()
        .filter(ConfigColumn::SourceKind.eq(source_kind))
        .filter(ConfigColumn::WorkspaceId.eq(workspace_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Refuse when the caller's platform grant does not reach `workspace_id`.
///
/// The shared fence (`admin::scope::deny_out_of_scope_opt`) is keyed by **org**,
/// and a workspace is not itself scopeable — so, exactly as
/// `workspaces_admin` does, this reads the workspace's owning org first and
/// fences on that. A missing workspace answers the same `404` an out-of-scope
/// one does, so the two are indistinguishable and the route cannot be used to
/// probe the workspace directory.
///
/// Resolving the workspace is not extra work bought only for the fence: the
/// `workspace_id` FK is `ON DELETE CASCADE`, so a `PUT` naming a workspace that
/// isn't there used to surface as a raw FK violation (`500`). It is now the
/// `404` it always was.
///
/// Named to end in `deny_out_of_scope`'s spelling on purpose — the same wrapper
/// shape `org_subdomains` uses — so the boundary scan in
/// `crates/app/tests/authz/app_scope_boundary.rs` can see the fence at the call
/// site rather than through one more indirection.
async fn deny_out_of_scope_for_workspace(
    db: &DatabaseConnection,
    actor: &AuthenticatedUser,
    workspace_id: Uuid,
) -> Result<(), Response> {
    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| error_body(StatusCode::NOT_FOUND, "workspace_not_found", None))?;
    scope::deny_out_of_scope_opt(db, actor, ws.org_id)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => error_body(status, "workspace_not_found", None),
            other => error_body(
                other,
                "scope_unreadable",
                Some("platform grant could not be read".into()),
            ),
        })
}

/// `PUT /api/admin/airway/config/{source_kind}`.
pub async fn put_global_config(
    Path(source_kind): Path<String>,
    Json(req): Json<UpsertConfigRequest>,
) -> Result<StatusCode, Response> {
    let db = connect().await?;
    upsert_global(
        &db,
        &source_kind,
        req.contract_policy.as_deref(),
        req.environment.as_deref(),
    )
    .await
    .map_err(config_write_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/admin/airway/config/{source_kind}`.
pub async fn delete_global_config(Path(source_kind): Path<String>) -> Result<StatusCode, Response> {
    let db = connect().await?;
    delete_global(&db, &source_kind)
        .await
        .map_err(config_write_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/admin/airway/config/{source_kind}/workspaces/{workspace_id}`.
///
/// Fences on the target workspace **before** writing: the capability layer
/// admits a scoped operator to this console but knows nothing about which
/// tenants they reach, so without this a grant bounded to two orgs could pin a
/// third tenant's pipelines to `require_declared`.
pub async fn put_workspace_override(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((source_kind, workspace_id)): Path<(String, Uuid)>,
    Json(req): Json<UpsertConfigRequest>,
) -> Result<StatusCode, Response> {
    let db = connect().await?;
    deny_out_of_scope_for_workspace(&db, &actor, workspace_id).await?;
    upsert_override(
        &db,
        &source_kind,
        workspace_id,
        req.contract_policy.as_deref(),
        req.environment.as_deref(),
    )
    .await
    .map_err(config_write_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/admin/airway/config/{source_kind}/workspaces/{workspace_id}`.
///
/// Fenced like the `PUT`. Removing an override is not the safe direction of the
/// pair — it drops the workspace back onto the global row, which may be
/// stricter than the override it replaces — so both verbs carry the same check.
pub async fn delete_workspace_override(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((source_kind, workspace_id)): Path<(String, Uuid)>,
) -> Result<StatusCode, Response> {
    let db = connect().await?;
    deny_out_of_scope_for_workspace(&db, &actor, workspace_id).await?;
    delete_override(&db, &source_kind, workspace_id)
        .await
        .map_err(config_write_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
