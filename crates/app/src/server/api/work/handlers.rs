use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use entity::{locations, org_role_members, org_roles, users, work_items};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use std::collections::HashMap;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::dto::*;
use crate::server::api::middlewares::role_guards::{OrgAdmin, OrgMemberStrict};

/// Every role the caller holds, as `(role_id, location_id)`.
///
/// This is what makes role-addressed work reachable: "the closing checklist"
/// is assigned to whoever is Shift Lead at Clovis, not to a person, so a query
/// that only looked at `assignee_user_id` would show that worker nothing.
///
/// A failure here must not degrade to "holds no roles": that returns `200` with
/// only the directly-assigned items, so a worker whose whole queue is
/// role-addressed sees an empty, authoritative-looking list instead of an error.
async fn roles_held(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<(Uuid, Option<Uuid>)>, DbErr> {
    Ok(org_role_members::Entity::find()
        .filter(org_role_members::Column::UserId.eq(user_id))
        .all(db)
        .await?
        .into_iter()
        .map(|m| (m.role_id, m.location_id))
        .collect())
}

/// Names for one item's ids.
///
/// Takes already-resolved maps rather than a connection, so the batched path
/// and the single-item path cannot disagree about how a name is looked up.
fn to_dto(
    w: work_items::Model,
    locations: &HashMap<Uuid, String>,
    users: &HashMap<Uuid, String>,
    roles: &HashMap<Uuid, String>,
) -> WorkItemDto {
    let now = Utc::now().fixed_offset();
    WorkItemDto {
        location_name: w.location_id.and_then(|id| locations.get(&id).cloned()),
        assignee_name: w.assignee_user_id.and_then(|id| users.get(&id).cloned()),
        assignee_role_name: w.assignee_role_id.and_then(|id| roles.get(&id).cloned()),
        id: w.id,
        title: w.title.clone(),
        body: w.body.clone(),
        org_id: w.org_id,
        location_id: w.location_id,
        assignee_user_id: w.assignee_user_id,
        assignee_role_id: w.assignee_role_id,
        supervisor_id: w.supervisor_id,
        due_at: w.due_at.map(|d| d.to_rfc3339()),
        status: w.status.clone(),
        priority: w.priority,
        source_kind: w.source_kind.clone(),
        source_id: w.source_id.clone(),
        overdue: w.is_overdue(now),
        created_at: w.created_at.to_rfc3339(),
        completed_at: w.completed_at.map(|d| d.to_rfc3339()),
    }
}

/// Resolve names for a whole page in three queries, not three per row.
///
/// The list caps at 200 items, so the per-row shape was up to 600 round trips
/// for one request — and each was a `find_by_id` for a name, which is exactly
/// the work an `IN (…)` does once.
async fn hydrate_all(
    db: &DatabaseConnection,
    items: Vec<work_items::Model>,
) -> Result<Vec<WorkItemDto>, DbErr> {
    let mut location_ids: Vec<Uuid> = items.iter().filter_map(|w| w.location_id).collect();
    let mut user_ids: Vec<Uuid> = items.iter().filter_map(|w| w.assignee_user_id).collect();
    let mut role_ids: Vec<Uuid> = items.iter().filter_map(|w| w.assignee_role_id).collect();
    // One row per distinct id — a page is usually a handful of locations
    // repeated, so the dedup is most of the saving.
    for v in [&mut location_ids, &mut user_ids, &mut role_ids] {
        v.sort_unstable();
        v.dedup();
    }

    let locations: HashMap<Uuid, String> = if location_ids.is_empty() {
        HashMap::new()
    } else {
        locations::Entity::find()
            .filter(locations::Column::Id.is_in(location_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|l| (l.id, l.name))
            .collect()
    };
    let users: HashMap<Uuid, String> = if user_ids.is_empty() {
        HashMap::new()
    } else {
        users::Entity::find()
            .filter(users::Column::Id.is_in(user_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|u| (u.id, u.name))
            .collect()
    };
    let roles: HashMap<Uuid, String> = if role_ids.is_empty() {
        HashMap::new()
    } else {
        org_roles::Entity::find()
            .filter(org_roles::Column::Id.is_in(role_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|r| (r.id, r.name))
            .collect()
    };

    Ok(items
        .into_iter()
        .map(|w| to_dto(w, &locations, &users, &roles))
        .collect())
}

/// One item, through the same path as a page of them.
async fn hydrate(db: &DatabaseConnection, w: work_items::Model) -> Result<WorkItemDto, DbErr> {
    Ok(hydrate_all(db, vec![w])
        .await?
        .pop()
        .expect("hydrate_all preserves length"))
}

/// `GET /api/work` — assigned to me, or supervised by me.
///
/// The filter is the authorization. There is no org gate here on purpose: a
/// frontline worker holds no `org_members` row by design, and the whole point
/// of assigning them work is that they can see it. A gate written as "is a
/// member of the org" would lock out exactly the people this graph exists to
/// route work to.
#[instrument(skip_all, fields(scope = ?q.scope))]
pub async fn list(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<WorkItemDto>>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    // Fetched before the match rather than inside it, because it is now
    // fallible and a `Condition` builder is no place to lose an error.
    let held = roles_held(&db, user.id).await.map_err(db_err)?;

    let mine = match q.scope {
        Scope::SupervisedByMe => Condition::all().add(work_items::Column::SupervisorId.eq(user.id)),
        Scope::AssignedToMe => {
            // Directly assigned, OR addressed to a role this person holds —
            // and for a location-scoped role, only at the location they hold
            // it at. Without that last clause a Shift Lead at Clovis would see
            // every store's closing checklist.
            let mut cond = Condition::any().add(work_items::Column::AssigneeUserId.eq(user.id));
            for (role_id, location_id) in held {
                let mut arm = Condition::all().add(work_items::Column::AssigneeRoleId.eq(role_id));
                if let Some(loc) = location_id {
                    arm = arm.add(work_items::Column::LocationId.eq(loc));
                }
                cond = cond.add(arm);
            }
            Condition::all().add(cond)
        }
    };

    let mut filter = mine;
    if !q.include_done {
        // Cancelled is closed, not outstanding. `<> 'done'` kept it in the
        // list, so a cancelled task went on reading as work somebody still owes
        // — the one thing a to-do list must not get wrong. Named positively
        // against the two open states rather than by exclusion, so a fifth
        // status added later is closed until somebody says otherwise. Still a
        // subset of the `status <> 'done'` partial indexes, so they apply.
        filter = filter.add(work_items::Column::Status.is_in(["open", "in_progress"]));
    }
    if let Some(loc) = q.location_id {
        filter = filter.add(work_items::Column::LocationId.eq(loc));
    }

    let rows = work_items::Entity::find()
        .filter(filter)
        // Oldest due first. An item with no due date is not urgent, and it
        // sorts last because Postgres already orders NULLs LAST for ASC — the
        // behaviour we want, relied on rather than restated. (An earlier
        // comment here had this backwards; NULLS FIRST is the DESC default.)
        .order_by_asc(work_items::Column::DueAt)
        .order_by_desc(work_items::Column::Priority)
        .limit(clamp_limit(q.limit))
        .all(&db)
        .await
        .map_err(|e| {
            warn!(error = %e, "work item list failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(hydrate_all(&db, rows).await.map_err(db_err)?))
}

/// Does the caller have ANY standing in this org?
///
/// Membership, or a tenant-defined role held there. Deliberately broader than
/// `org_members`: a frontline worker holds no membership row by design, and
/// this is the check that has to admit them without admitting everybody.
///
/// It exists because `/work` is mounted OUTSIDE `/orgs/{org_id}` — nesting it
/// would put `org_middleware` in front, which rejects exactly those workers. So
/// the gate `org_middleware` would have provided has to be made here instead,
/// explicitly, rather than being absent because the route moved.
///
/// Errors propagate rather than reading as "no standing". Fail-closed is right
/// for the decision, but collapsing a transient fault into `false` answers a
/// create with `404 no such org` during a blip — the least legible failure
/// available, and one that reads to the caller as data loss.
pub async fn has_standing_in_org(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<bool, DbErr> {
    let member = entity::org_members::Entity::find()
        .filter(entity::org_members::Column::OrgId.eq(org_id))
        .filter(entity::org_members::Column::UserId.eq(user_id))
        .one(db)
        .await?
        .is_some();
    if member {
        return Ok(true);
    }
    Ok(org_role_members::Entity::find()
        .filter(org_role_members::Column::OrgId.eq(org_id))
        .filter(org_role_members::Column::UserId.eq(user_id))
        .one(db)
        .await?
        .is_some())
}

/// Is this the unique-constraint collision, rather than any other failure?
///
/// Sea-ORM does not model constraint kinds, so the driver's SQLSTATE is the only
/// thing that distinguishes "this name is taken" from "the database is down".
fn is_unique_violation(e: &DbErr) -> bool {
    crate::server::api::operating_graph::is_unique_violation(e)
}

/// Map a lookup failure to a 500 that says so, rather than to a refusal.
fn db_err(e: DbErr) -> StatusCode {
    warn!(error = %e, "work authorization lookup failed");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Every caller-supplied id on a create, checked against the target org.
///
/// Separated from the handler because it IS the authorization: an authz
/// decision reachable only through an extractor is one no test covers, and this
/// endpoint has already shipped a cross-tenant write once — the first fix
/// covered four of the five ids.
///
/// The refusals differ on purpose. A bad `org_id` is `404`, because an org the
/// caller has no standing in must not be confirmed to exist by the shape of the
/// refusal. Everything else is `400`: the caller can already see the org, so a
/// wrong id there is a fact about their request, not a boundary.
pub async fn gate_create(
    db: &DatabaseConnection,
    caller: Uuid,
    body: &CreateWorkItem,
) -> Result<(), StatusCode> {
    if !has_standing_in_org(db, caller, body.org_id)
        .await
        .map_err(db_err)?
    {
        warn!(user = %caller, org = %body.org_id, "work create refused — no standing in org");
        return Err(StatusCode::NOT_FOUND);
    }
    // The assignee must be in the same org. Gating only the caller would still
    // let a member of one tenant address work at somebody in another.
    if let Some(assignee) = body.assignee_user_id
        && !has_standing_in_org(db, assignee, body.org_id)
            .await
            .map_err(db_err)?
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    // The supervisor is an assignment edge too, and the one the original gate
    // missed: `Scope::SupervisedByMe` filters on `supervisor_id` with no org
    // predicate, so an unchecked uuid here lands attacker-authored work in a
    // stranger's "supervised by me" — in any tenant. The `users(id)` FK proves
    // the person exists, never that they are in this org.
    if let Some(sup) = body.supervisor_id
        && !has_standing_in_org(db, sup, body.org_id)
            .await
            .map_err(db_err)?
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    // A role has to belong to this org too — a role id is a uuid a caller
    // supplies, and one from another tenant would route work across the
    // boundary just as effectively as a user id.
    if let Some(role) = body.assignee_role_id
        && !org_roles::Entity::find_by_id(role)
            .one(db)
            .await
            .map_err(db_err)?
            .is_some_and(|r| r.org_id == body.org_id)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Same for the location: it names where the work happens, and one from
    // another org would leak this item into that org's location view.
    if let Some(loc) = body.location_id
        && !locations::Entity::find_by_id(loc)
            .one(db)
            .await
            .map_err(db_err)?
            .is_some_and(|l| l.org_id == body.org_id)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

/// `POST /api/work` — create an item.
///
/// # The gate, and why it is here rather than in middleware
///
/// The caller must have standing in the target org, and so must anyone they
/// assign to. Without both, this endpoint is an injection point: `org_id` and
/// `assignee_user_id` arrive in the BODY, so an authenticated user of any
/// tenant could otherwise create work in another tenant's org and push it into
/// a stranger's "assigned to me" — a cross-tenant write that then reads back as
/// a legitimate task.
///
/// The assignee check is the half that is easy to forget: gating only the org
/// would still let a member of a shared org address work to somebody who has
/// since left it.
#[instrument(skip_all, fields(org_id = %body.org_id))]
pub async fn create(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<CreateWorkItem>,
) -> Result<(StatusCode, Json<WorkItemDto>), StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    gate_create(&db, user.id, &body).await?;

    let title = body.title.trim();
    if title.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Checked here rather than left to the database: the constraint returns a
    // 500-shaped error, and "you assigned this to nobody" deserves a 400 that
    // says so.
    if body.assignee_user_id.is_none() && body.assignee_role_id.is_none() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let due = match body.due_at.as_deref() {
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(d) => Some(d),
            Err(_) => return Err(StatusCode::BAD_REQUEST),
        },
        None => None,
    };

    let saved = work_items::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(body.org_id),
        location_id: Set(body.location_id),
        title: Set(title.to_string()),
        body: Set(body.body.clone()),
        assignee_user_id: Set(body.assignee_user_id),
        assignee_role_id: Set(body.assignee_role_id),
        supervisor_id: Set(body.supervisor_id),
        due_at: Set(due),
        status: Set("open".to_string()),
        priority: Set(body.priority),
        source_kind: Set(body.source_kind.clone()),
        source_id: Set(body.source_id.clone()),
        created_by: Set(Some(user.id)),
        created_at: Set(Utc::now().fixed_offset()),
        updated_at: Set(Utc::now().fixed_offset()),
        completed_at: Set(None),
        completed_by: Set(None),
    }
    .insert(&db)
    .await
    .map_err(|e| {
        warn!(error = %e, "work item insert failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!(id = %saved.id, "work item created");
    Ok((
        StatusCode::CREATED,
        Json(hydrate(&db, saved).await.map_err(db_err)?),
    ))
}

/// `PATCH /api/work/{id}` — complete it, reassign it, move the date.
#[instrument(skip_all, fields(id = %id))]
pub async fn update(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateWorkItem>,
) -> Result<Json<WorkItemDto>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let Some(item) = work_items::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(StatusCode::NOT_FOUND);
    };

    // Assignee, supervisor, or holder of the addressed role. Same rule as the
    // read, restated for the write — and 404 rather than 403, so an item the
    // caller cannot touch is indistinguishable from one that does not exist.
    let role_match = match item.assignee_role_id {
        Some(role) => roles_held(&db, user.id)
            .await
            .map_err(db_err)?
            .iter()
            .any(|(r, loc)| *r == role && (loc.is_none() || *loc == item.location_id)),
        None => false,
    };
    if item.assignee_user_id != Some(user.id) && item.supervisor_id != Some(user.id) && !role_match
    {
        return Err(StatusCode::NOT_FOUND);
    }

    let now = Utc::now().fixed_offset();
    // Read before `item` is consumed by the conversion below.
    let item_org = item.org_id;
    let mut update: work_items::ActiveModel = item.into();
    update.updated_at = Set(now);

    if let Some(status) = body.status.as_deref() {
        if !is_settable_status(status) {
            return Err(StatusCode::BAD_REQUEST);
        }
        update.status = Set(status.to_string());
        // The schema requires completion to be whole — status and timestamp
        // move together or the CHECK rejects the row. Setting both here means
        // a caller cannot produce a "done" item nobody appears to have done.
        if status == "done" {
            update.completed_at = Set(Some(now));
            update.completed_by = Set(Some(user.id));
        } else {
            update.completed_at = Set(None);
            update.completed_by = Set(None);
        }
    }
    // Same gate as `create`. Without it one legitimately-created item plus one
    // PATCH reaches exactly what `create` refuses: an arbitrary global uuid in
    // `assignee_user_id`, which `Scope::AssignedToMe` then surfaces with no org
    // predicate of its own. A gate a single follow-up request walks around is
    // not a gate.
    if let Some(a) = body.assignee_user_id {
        if !has_standing_in_org(&db, a, item_org)
            .await
            .map_err(db_err)?
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        update.assignee_user_id = Set(Some(a));
    }
    if let Some(d) = body.due_at.as_deref() {
        match DateTime::parse_from_rfc3339(d) {
            Ok(parsed) => update.due_at = Set(Some(parsed)),
            Err(_) => return Err(StatusCode::BAD_REQUEST),
        }
    }

    let saved = update.update(&db).await.map_err(|e| {
        warn!(error = %e, "work item update failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(hydrate(&db, saved).await.map_err(db_err)?))
}

// ── Locations and roles ────────────────────────────────────────────────────
//
// A different authority from doing the work: these decide the SHAPE of the org,
// so unlike the reads above they go through the model.

/// `POST /api/orgs/{org_id}/locations` — org owner/admin only.
///
/// The org is in the PATH, not the body, and that is deliberate: `OrgAdmin`
/// resolves from the request's `OrgContext`, so a body-carried org is invisible
/// to it. `create_app` documents that exact hole as "scope exception #1" and
/// has to re-check by hand; putting the org where the guard can see it means
/// there is nothing to remember.
#[instrument(skip_all, fields(org_id = %org_id))]
pub async fn create_location(
    OrgAdmin(_ctx): OrgAdmin,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateLocation>,
) -> Result<
    (
        StatusCode,
        Json<crate::server::api::operating_graph::dto::LocationRow>,
    ),
    StatusCode,
> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // A location's timezone decides which local day a shift, a checklist and an
    // overdue calculation fall on, so an unparseable one is not cosmetic — it
    // silently moves work to the wrong day. Rejected here rather than stored and
    // discovered later.
    let timezone = body.timezone.clone().unwrap_or_else(|| "UTC".to_string());
    if timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let status = body.status.clone().unwrap_or_else(|| "open".to_string());
    if !crate::server::api::operating_graph::dto::is_location_status(&status) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = Uuid::new_v4();
    // The hierarchy is one self-reference, checked by the same rule PATCH
    // uses: this org's, and not a loop.
    let parent_id = crate::server::api::operating_graph::locations::check_parent(
        &db,
        org_id,
        id,
        body.parent_id,
    )
    .await
    .map_err(|e| e.status())?;
    let kind = body
        .kind
        .as_deref()
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty());

    // The tenant's own id, by the rule PATCH uses: trimmed, blank is none, and
    // no other place of the org carries it (409).
    let external_id = crate::server::api::operating_graph::locations::check_external_id(
        &db,
        org_id,
        id,
        body.external_id.clone(),
    )
    .await
    .map_err(|e| e.status())?;

    let now = Utc::now().fixed_offset();
    let saved = locations::ActiveModel {
        id: Set(id),
        org_id: Set(org_id),
        name: Set(name.to_string()),
        status: Set(status),
        timezone: Set(timezone),
        external_id: Set(external_id),
        parent_id: Set(parent_id),
        kind: Set(kind),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .map_err(|e| {
        // 409 only for the collision this table can actually have — a duplicate
        // `(org_id, name)`; the tenant id was checked above. Answering every
        // insert failure with CONFLICT tells an operator to go looking for a
        // clashing row that is not there.
        if is_unique_violation(&e) {
            StatusCode::CONFLICT
        } else {
            warn!(error = %e, "location insert failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;

    info!(id = %saved.id, "location created");
    // The same shape `GET` and `PATCH` answer — with `external_ids`, empty
    // here — so a client can treat every location it holds alike.
    match crate::server::api::operating_graph::locations::one_row(&db, org_id, saved.id).await {
        Ok(Some(row)) => Ok((StatusCode::CREATED, Json(row))),
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// `GET /api/orgs/{org_id}/roles` — any member: a picker needs the vocabulary.
#[instrument(skip_all, fields(org_id = %org_id))]
pub async fn list_roles(
    OrgMemberStrict(_ctx): OrgMemberStrict,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Vec<org_roles::Model>>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let rows = org_roles::Entity::find()
        .filter(org_roles::Column::OrgId.eq(org_id))
        .order_by_asc(org_roles::Column::Name)
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows))
}

/// `POST /api/orgs/{org_id}/roles` — org owner/admin only. Same path-not-body
/// reasoning as `create_location`.
#[instrument(skip_all, fields(org_id = %org_id))]
pub async fn create_role(
    OrgAdmin(_ctx): OrgAdmin,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateRole>,
) -> Result<(StatusCode, Json<org_roles::Model>), StatusCode> {
    if !matches!(body.scope.as_str(), "location" | "franchisor") {
        return Err(StatusCode::BAD_REQUEST);
    }
    // An unnamed role is unusable: it is what work gets addressed to, and it
    // shows up in a picker as a blank line. `create_location` already refuses
    // an empty name; this is the same rule.
    let name = body.name.trim();
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;

    let now = Utc::now().fixed_offset();
    let saved = org_roles::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name.to_string()),
        scope: Set(body.scope.clone()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .map_err(|e| {
        // Same rule as `create_location`: 409 only for the collision this table
        // can actually have. `org_roles` carries the same `UNIQUE (org_id,
        // name)` shape, so the two functions must answer alike — otherwise a
        // database blip on role creation sends an operator looking for a
        // clashing role name that is not there.
        if is_unique_violation(&e) {
            StatusCode::CONFLICT
        } else {
            warn!(error = %e, "org role insert failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;

    info!(id = %saved.id, "org role created");
    Ok((StatusCode::CREATED, Json(saved)))
}
