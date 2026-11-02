//! Staff-facing Airhouse fleet, for the admin console.
//!
//! The member-facing `/airhouse/me/*` routes answer "what is MY workspace's
//! warehouse". This answers the operator's question instead: which workspaces
//! have one, which do not, and is anything wrong with the ones that do.
//!
//! **Workspace-keyed, not org-keyed** — an Airhouse tenant is one per
//! workspace, not one per org. That is why the scope fence on the
//! `oxy-app` side uses `deny_out_of_scope_opt`: a workspace's org is nullable,
//! and a null org must refuse for a bounded grant rather than pass unchecked.
//!
//! There is deliberately **no `router()` here**: the routes are
//! declared by `oxy_app::server::api::admin::airhouse`, which fences each one
//! before delegating. The capability layer at the mount decides *whether you
//! are staff*; only a handler can decide *which orgs you reach*.

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_platform::db::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Serialize;
use tracing::{error, info, instrument};
use uuid::Uuid;

use crate::entity::tenants::Entity as AirhouseTenants;

/// Ceiling on one fleet read. Large enough that no real deployment notices,
/// small enough that a runaway one cannot page the whole workspaces table into
/// a JSON response.
///
/// Paired with an ORDER BY and a `truncated` flag. A cap on an unordered query
/// returns an arbitrary subset — Postgres has no row order without ORDER BY, so
/// two loads of the same page could differ.
///
/// **The cut falls on unprovisioned workspaces only.** Ordering by name and
/// then sorting provisioned-first in memory reordered the page without
/// changing which rows it held, so a tenant on a workspace whose name sorted
/// past the cut was simply absent — and absent reads identically to "has no
/// warehouse", which is the one thing this page exists to tell apart. Every
/// provisioned workspace is loaded by id from the tenants table first; the
/// remaining budget goes to workspaces that have none.
const MAX_FLEET_ROWS: u64 = 500;

/// How many unprovisioned workspaces this page can still afford.
///
/// The provisioned half is loaded first and in full, so it spends the budget
/// before the rest of the fleet sees any. Saturating: a deployment with more
/// warehouses than the cap leaves nothing, rather than wrapping into a huge
/// limit.
fn unprovisioned_budget(provisioned: usize) -> u64 {
    MAX_FLEET_ROWS.saturating_sub(provisioned as u64)
}

/// Which halves of the page are incomplete.
///
/// Two ways to overflow, and they need **different** words on screen, which is
/// why this is not one boolean. The unprovisioned query fetched one past its
/// budget (`fetched > budget`) — every warehouse is still shown, and only the
/// "no warehouse" list is a prefix. Or the provisioned half alone reached the
/// cap, in which case its own `LIMIT` may have cut warehouses off, and any copy
/// promising "every provisioned workspace is shown" asserts the opposite of
/// what happened. Reporting only the first is what makes a bound read as a
/// complete fleet.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FleetTruncation {
    /// Workspaces without a warehouse were cut. The provisioned half is whole.
    pub unprovisioned: bool,
    /// The provisioned half hit its own cap, so a warehouse may be missing.
    pub provisioned: bool,
}

impl FleetTruncation {
    /// Whether anything at all is hidden.
    pub fn any(&self) -> bool {
        self.unprovisioned || self.provisioned
    }
}

fn truncation(fetched_unprovisioned: usize, budget: u64, tenants: usize) -> FleetTruncation {
    FleetTruncation {
        unprovisioned: fetched_unprovisioned as u64 > budget,
        provisioned: tenants as u64 >= MAX_FLEET_ROWS,
    }
}

/// One row of the fleet: a workspace, and its warehouse if it has one.
#[derive(Debug, Serialize)]
pub struct AirhouseFleetRow {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub org_id: Option<Uuid>,
    pub org_name: String,
    /// `none` when the workspace has no tenant yet.
    pub status: String,
    /// Airhouse-side tenant id — the name an operator gives their admin API.
    pub tenant_id: String,
    pub bucket: String,
    pub prefix: String,
    /// Whether a service account is bound. Without one the workspace cannot
    /// mint the ephemeral credentials every query uses, so it is provisioned
    /// in name only.
    pub service_account_ready: bool,
    /// `None` when the SA has never been rotated.
    pub sa_rotated_at: Option<String>,

    // Everything below is already on the row this query loads and was
    // discarded. It is also the entire content of the psql session an operator
    // opens when this page cannot answer their question — which is the page
    // failing at its job, not the operator being thorough.
    /// When the tenant was provisioned.
    ///
    /// Pairs with `sa_created_at` to make "never rotated" mean something. On
    /// its own that phrase reads identically for a tenant provisioned this
    /// morning and one provisioned two years ago, and only the second is a
    /// finding.
    pub created_at: Option<String>,
    /// The service account's Airhouse-side id — what an operator gives the
    /// Airhouse admin API to look at this tenant from the other side.
    pub service_account_id: Option<String>,
    /// When the service account was bound.
    pub sa_created_at: Option<String>,
    // The service account's **ceilings**, not the credential a caller gets.
    // Both are written once at provisioning from `SA_MAX_ROLE` / `SA_MAX_TTL_SECS`
    // and never varied, so on a healthy fleet every row reads `admin` / 86400.
    // The effective role and TTL are chosen per mint by the broker from the
    // caller's org role (Owner→admin, Admin→writer, Member→reader), and this
    // page does not carry that.
    //
    // Worth surfacing anyway for the inverse reading: a row that does NOT match
    // the constants is itself the finding — a tenant provisioned under an older
    // policy, or one whose SA was rotated against a different cap.
    /// The strongest role a minted credential may carry.
    pub bearer_max_role: Option<String>,
    /// The longest a minted credential may live.
    pub bearer_max_ttl_secs: Option<i32>,
}

/// The fleet, and whether it is all of it.
#[derive(Debug, Serialize)]
pub struct AirhouseFleet {
    pub rows: Vec<AirhouseFleetRow>,
    /// Which halves are incomplete, by cause — the page says so rather than
    /// presenting a partial fleet as a complete one, and the two causes need
    /// different words.
    pub truncated: FleetTruncation,
}

/// One workspace's row. Shared so the list and the post-provision read-back
/// cannot disagree about what a row means.
fn row_from(
    workspace: &entity::workspaces::Model,
    org_name: String,
    tenant: Option<&crate::entity::tenants::Model>,
) -> AirhouseFleetRow {
    match tenant {
        None => AirhouseFleetRow {
            workspace_id: workspace.id,
            workspace_name: workspace.name.clone(),
            org_id: workspace.org_id,
            org_name,
            status: "none".to_string(),
            tenant_id: String::new(),
            bucket: String::new(),
            prefix: String::new(),
            service_account_ready: false,
            sa_rotated_at: None,
            created_at: None,
            service_account_id: None,
            sa_created_at: None,
            bearer_max_role: None,
            bearer_max_ttl_secs: None,
        },
        Some(t) => AirhouseFleetRow {
            workspace_id: workspace.id,
            workspace_name: workspace.name.clone(),
            org_id: workspace.org_id,
            org_name,
            status: t.status.as_str().to_string(),
            tenant_id: t.airhouse_tenant_id.clone(),
            bucket: t.bucket.clone(),
            prefix: t.prefix.clone().unwrap_or_default(),
            // The ciphertext, not just the id: a row can carry an SA id whose
            // bearer was never sealed, and that workspace cannot mint anything.
            service_account_ready: t.service_account_id.is_some() && t.bearer_ciphertext.is_some(),
            sa_rotated_at: t.sa_rotated_at.map(|d| d.to_rfc3339()),
            created_at: Some(t.created_at.to_rfc3339()),
            service_account_id: t.service_account_id.clone(),
            sa_created_at: t.sa_created_at.map(|d| d.to_rfc3339()),
            bearer_max_role: t.bearer_max_role.clone(),
            bearer_max_ttl_secs: t.bearer_max_ttl_secs,
        },
    }
}

fn internal<E: std::fmt::Display>(what: &'static str) -> impl Fn(E) -> StatusCode {
    move |e| {
        error!("airhouse admin: {what}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// Every workspace, with its Airhouse tenant when it has one.
///
/// Both halves matter: a list of existing tenants cannot answer "who still
/// needs one", which is the question an operator usually arrives with.
///
/// A fixed number of queries joined in memory, not one per workspace: counting
/// inside the row loop pays 201 queries to render 200 rows.
///
/// `scope` is the orgs the caller may see, or `None` for unbounded, and it is
/// applied IN the query rather than to the result. The shim used to filter the
/// returned `Vec`: correct, but a bounded operator paid for every workspace on
/// the deployment, and `workspaces` is the largest of the three tables here.
// `&AuthenticatedUser`, not the extractor: this stopped being an axum handler
// when the shim took over the route, and a second non-extractor argument beside
// one is a shape axum would reject anyway. It is here for the span field.
#[instrument(skip(user, scope), fields(user_id = %user.id))]
pub async fn list_fleet(
    user: &oxy_auth::types::AuthenticatedUser,
    scope: Option<&[Uuid]>,
) -> Result<Json<AirhouseFleet>, StatusCode> {
    use std::collections::HashMap;

    let db = establish_connection().await.map_err(internal("connect"))?;

    // `scope` is applied IN every query rather than to the result. The shim used
    // to filter the returned `Vec`: correct, but a bounded operator paid for
    // every workspace on the deployment, and `workspaces` is the largest table
    // here. A workspace with no org cannot be in `Scope::Orgs(..)`, so a
    // bounded grant does not see it — the same direction the write-path fence
    // takes.
    let scoped = |q: sea_orm::Select<entity::prelude::Workspaces>| match scope {
        Some(orgs) => q.filter(entity::workspaces::Column::OrgId.is_in(orgs.to_vec())),
        None => q,
    };

    // The provisioned set first, and by id — it is the bounded half (one row
    // per warehouse that exists) and the half an operator acts on, so it is
    // never what a cap drops.
    //
    // **Scoped, like everything else.** An unscoped read here took the first
    // `MAX_FLEET_ROWS` tenants deployment-wide, and for a bounded operator that
    // prefix need not contain any of their orgs' tenants at all: `provisioned`
    // came back empty, the whole budget went to the unprovisioned query, and
    // their warehouses were fetched by it and rendered `status: "none"` with a
    // Provision button. That is worse than the absence this split was written
    // to fix — the row positively asserts the wrong state, and the action it
    // offers is one the operator has no reason to doubt.
    let tenants: HashMap<Uuid, crate::entity::tenants::Model> = match scope {
        Some(orgs) => AirhouseTenants::find().filter(
            crate::entity::tenants::Column::WorkspaceId.in_subquery(
                sea_orm::sea_query::Query::select()
                    .column(entity::workspaces::Column::Id)
                    .from(entity::workspaces::Entity)
                    .and_where(entity::workspaces::Column::OrgId.is_in(orgs.to_vec()))
                    .to_owned(),
            ),
        ),
        None => AirhouseTenants::find(),
    }
    .order_by_asc(crate::entity::tenants::Column::WorkspaceId)
    .limit(MAX_FLEET_ROWS)
    .all(&db)
    .await
    .map_err(internal("list airhouse tenants"))?
    .into_iter()
    .map(|t| (t.workspace_id, t))
    .collect();
    let provisioned_ids: Vec<Uuid> = tenants.keys().copied().collect();

    let provisioned = scoped(entity::prelude::Workspaces::find())
        .filter(entity::workspaces::Column::Id.is_in(provisioned_ids.clone()))
        .order_by_asc(entity::workspaces::Column::Name)
        .order_by_asc(entity::workspaces::Column::Id)
        .all(&db)
        .await
        .map_err(internal("list provisioned workspaces"))?;

    // Whatever budget the provisioned rows left, spent on workspaces that still
    // need one — ordered in SQL, and one past the cap so "there are more" is a
    // fact rather than a guess.
    //
    // Tie-break on id: `name` has no unique index, so rows sharing one are
    // unordered relative to each other — including across the cut, which is the
    // same "two loads differ" this ordering exists to stop.
    let remaining = unprovisioned_budget(provisioned.len());
    let mut unprovisioned = if remaining == 0 {
        Vec::new()
    } else {
        scoped(entity::prelude::Workspaces::find())
            .filter(entity::workspaces::Column::Id.is_not_in(provisioned_ids))
            .order_by_asc(entity::workspaces::Column::Name)
            .order_by_asc(entity::workspaces::Column::Id)
            .limit(remaining + 1)
            .all(&db)
            .await
            .map_err(internal("list unprovisioned workspaces"))?
    };
    let truncated = truncation(unprovisioned.len(), remaining, tenants.len());
    unprovisioned.truncate(remaining as usize);

    let workspaces: Vec<entity::workspaces::Model> =
        provisioned.into_iter().chain(unprovisioned).collect();

    // Keyed to the workspaces actually returned, so the lookup does not scan
    // past the page.
    let org_ids: Vec<Uuid> = workspaces.iter().filter_map(|w| w.org_id).collect();
    let orgs: HashMap<Uuid, String> = entity::prelude::Organizations::find()
        .filter(entity::organizations::Column::Id.is_in(org_ids))
        .all(&db)
        .await
        .map_err(internal("list orgs"))?
        .into_iter()
        .map(|o| (o.id, o.name))
        .collect();

    let mut rows: Vec<AirhouseFleetRow> = workspaces
        .iter()
        .map(|w| {
            let org_name = w
                .org_id
                .and_then(|id| orgs.get(&id).cloned())
                .unwrap_or_default();
            row_from(w, org_name, tenants.get(&w.id))
        })
        .collect();

    // Provisioned first, then alphabetical. Presentation only — the two queries
    // above already decided which rows exist, and they are concatenated in this
    // order; this makes that explicit rather than relying on it.
    rows.sort_by(|a, b| {
        (a.status == "none")
            .cmp(&(b.status == "none"))
            .then(a.workspace_name.cmp(&b.workspace_name))
    });
    Ok(Json(AirhouseFleet { rows, truncated }))
}

/// Provision (or reconcile) one workspace's Airhouse tenant.
///
/// Idempotent — a double-click, a retry after a timeout, or two operators
/// racing all converge on one tenant.
#[instrument(skip(user), fields(user_id = %user.id, workspace_id = %workspace_id))]
pub async fn provision(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<AirhouseFleetRow>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Airhouse not configured on this deployment is a normal state, not a
    // fault, so 503 (try a deployment that has it) rather than 500.
    let provisioner = crate::config::provisioner_for(db.clone()).ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Airhouse is not configured on this deployment".to_string(),
    ))?;

    let workspace = entity::prelude::Workspaces::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "workspace not found".to_string()))?;

    info!(user = %user.label(), workspace_id = %workspace_id, "provisioning airhouse tenant");
    provisioner
        .provision(workspace_id, tenant_name_for(&workspace))
        .await
        .map_err(|e| match e {
            // A name collision is the caller's problem to resolve, not an Oxy
            // fault — the member-facing handler already draws this line.
            crate::provisioner::ProvisionerError::TenantNameTaken(_) => {
                (StatusCode::CONFLICT, e.to_string())
            }
            crate::provisioner::ProvisionerError::InvalidTenantName(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, e.to_string())
            }
            other => (StatusCode::BAD_GATEWAY, other.to_string()),
        })?;

    // One row, by id. This read back through `list_fleet` — every workspace,
    // every organization and every tenant, sorted, then `.find()` — inside the
    // request, immediately after a provisioner call that already talked to the
    // Airhouse API. Using a fleet scan as a single-row read gives back exactly
    // what the two-queries-not-N+1 shape below was for.
    let tenant = AirhouseTenants::find()
        .filter(crate::entity::tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "provisioned but no tenant row on read-back".to_string(),
        ))?;
    let org_name = match workspace.org_id {
        Some(id) => entity::prelude::Organizations::find_by_id(id)
            .one(&db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map(|o| o.name)
            .unwrap_or_default(),
        None => String::new(),
    };
    Ok(Json(row_from(&workspace, org_name, Some(&tenant))))
}

/// The Airhouse-side tenant name for a workspace.
///
/// Derived, not asked for: the member-facing flow lets a user pick one because
/// they are naming their own warehouse, but an operator provisioning on someone
/// else's behalf has no basis for choosing, and a typo here is a global name
/// consumed forever. The workspace id is unique and already the key.
fn tenant_name_for(workspace: &entity::workspaces::Model) -> String {
    tenant_name_for_workspace(workspace.id)
}

/// The same derivation, by id — so an audit entry can record the name that was
/// attempted without loading the workspace again. Lives beside the provisioner,
/// which provisions under it when a workspace is created.
pub use crate::provisioner::tenant_name_for_workspace;

#[cfg(test)]
mod fleet_page_tests {
    use super::*;

    /// The invariant the two-query split exists for: a provisioned workspace is
    /// never the row a cap drops. It used to be — the read was ordered by name
    /// and cut at the cap, then sorted provisioned-first in memory, which
    /// reorders a page without changing which rows it holds. A tenant whose
    /// workspace name sorted past the cut was simply absent, and absent renders
    /// identically to "has no warehouse".
    #[test]
    fn the_provisioned_half_is_never_what_the_cap_drops() {
        // A fleet that is nearly all warehouses still shows every one of them.
        assert_eq!(unprovisioned_budget(MAX_FLEET_ROWS as usize - 1), 1);
        // And one past the cap spends nothing on the rest.
        assert_eq!(unprovisioned_budget(MAX_FLEET_ROWS as usize + 50), 0);
    }

    #[test]
    fn a_fleet_inside_the_cap_is_not_reported_as_truncated() {
        let budget = unprovisioned_budget(10);
        assert_eq!(budget, MAX_FLEET_ROWS - 10);
        // Exactly the budget: the +1 probe row came back empty.
        assert!(!truncation(budget as usize, budget, 10).any());
    }

    /// The `+ 1` probe: fetching one past the budget is how "there are more"
    /// becomes a fact rather than a guess.
    #[test]
    fn one_row_past_the_budget_reports_truncated() {
        let budget = unprovisioned_budget(10);
        let t = truncation(budget as usize + 1, budget, 10);
        assert!(t.unprovisioned, "the cut fell on the unprovisioned half");
        assert!(
            !t.provisioned,
            "every warehouse is still shown — saying otherwise would send an \
             operator looking for a row that is on the page"
        );
    }

    /// The second overflow path, and the reason this is not one boolean. With
    /// the budget at zero the probe cannot fire — `fetched` is also zero — so
    /// only the tenant count can tell the page it is incomplete, and what is
    /// incomplete is the half a single flag would have promised was whole.
    #[test]
    fn a_fleet_of_warehouses_past_the_cap_reports_the_provisioned_half() {
        let tenants = MAX_FLEET_ROWS as usize;
        let budget = unprovisioned_budget(tenants);
        assert_eq!(budget, 0, "no budget left, so no probe row to detect");
        let t = truncation(0, budget, tenants);
        assert!(
            t.any(),
            "a fleet at the cap must not claim to be the whole fleet"
        );
        assert!(
            t.provisioned,
            "the provisioned half is the one that was cut — copy promising \
             `every provisioned workspace is shown` asserts the opposite of \
             what happened"
        );
    }
}
