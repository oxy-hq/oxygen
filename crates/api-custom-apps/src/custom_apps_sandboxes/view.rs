//! Turning environment rows into the [`EnvironmentDto`] the API returns: one
//! query each for the builds, the owners' emails and the sandboxes' last
//! invocations, however many environments are shown.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use entity::{app_builds, app_environments, apps, users};
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use oxy_app_core::custom_apps_host_dispatch::environment_url_for;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use super::copies::{copies_expire_at, holding_copies, overlay_key_of};
use super::oltp_branch_dto::OltpBranchDto;
use super::{EnvironmentDto, OwnerDto, activity, oltp_state};
use crate::custom_apps_planes::{PlaneSpec, default_parent};

/// What the rows of one app need looked up to be shown.
pub(super) struct View<'a> {
    app: &'a apps::Model,
    org_slug: &'a str,
    /// `app_builds.id` → (the publish's build id, the build's semantic pin).
    builds: HashMap<Uuid, (String, Option<Uuid>)>,
    emails: HashMap<Uuid, Option<String>>,
    last_invocations: HashMap<String, DateTime<Utc>>,
    ttl: chrono::Duration,
    /// The org's OLTP staging branch as it is cut now, when it is active:
    /// what a sandbox's recorded schema is fresh or stale against.
    oltp_cut: Option<oxy_oltp::branches::BranchCut>,
    /// The app's environment branches by environment name.
    oltp_branches: HashMap<String, OltpBranchDto>,
    /// The overlay keys of the shown sandboxes whose Airhouse overlay still
    /// lists a table it copied.
    holding: HashSet<String>,
}

impl<'a> View<'a> {
    /// Look up everything `rows` and the two `fixed` builds reference.
    pub(super) async fn load(
        db: &DatabaseConnection,
        app: &'a apps::Model,
        org_slug: &'a str,
        rows: &[app_environments::Model],
        fixed: [Option<Uuid>; 2],
    ) -> Result<View<'a>, DbErr> {
        let build_ids: Vec<Uuid> = rows
            .iter()
            .filter_map(|row| row.build_id)
            .chain(fixed.into_iter().flatten())
            .collect();
        let owner_ids: Vec<Uuid> = rows.iter().filter_map(|row| row.owner_user_id).collect();
        Ok(View {
            app,
            org_slug,
            builds: load_builds(db, build_ids).await?,
            emails: load_emails(db, owner_ids).await?,
            last_invocations: activity::last_invocations(db, app.id).await?,
            ttl: super::idle_ttl(),
            oltp_cut: oltp_cut(db, app, rows).await?,
            oltp_branches: oltp_branches(db, app, rows).await?,
            holding: holding(db, rows).await?,
        })
    }

    /// `production` or `staging`: `row` when it exists (its timestamps), and
    /// `build` as resolution answers it — a fixed environment with no row
    /// still exists, and serves what its legacy column says.
    pub(super) fn fixed(
        &self,
        environment: &AppEnvironment,
        row: Option<&app_environments::Model>,
        build: Option<Uuid>,
    ) -> EnvironmentDto {
        let (created_at, updated_at) = match row {
            Some(row) => (row.created_at, row.updated_at),
            None => (self.app.created_at, self.app.updated_at),
        };
        self.environment(
            environment,
            row,
            build,
            created_at.with_timezone(&Utc),
            updated_at.with_timezone(&Utc),
        )
    }

    /// A sandbox's row, deleting or not.
    pub(super) fn sandbox(&self, row: &app_environments::Model) -> Option<EnvironmentDto> {
        let environment = AppEnvironment::parse(&row.name)?;
        let updated_at = row.updated_at.with_timezone(&Utc);
        let last_activity =
            activity::last_activity(updated_at, self.last_invocations.get(&row.name).copied());
        let mut dto = self.environment(
            &environment,
            Some(row),
            row.build_id,
            row.created_at.with_timezone(&Utc),
            updated_at,
        );
        if row.deleting_at.is_some() {
            dto.status = "deleting".to_string();
        }
        dto.owner = row.owner_user_id.map(|user_id| OwnerDto {
            user_id,
            email: self.emails.get(&user_id).cloned().flatten(),
        });
        dto.last_activity_at = Some(last_activity);
        dto.expires_at = Some(activity::expires_at(last_activity, self.ttl));
        dto.oltp_schema = oltp_state::of_row(row)
            .map(|state| oltp_state::OltpSchemaDto::of(state, self.oltp_cut.as_ref()));
        dto.oltp_branch = self.oltp_branches.get(&row.name).cloned();
        let holding = overlay_key_of(row).is_some_and(|key| self.holding.contains(&key));
        dto.copies_expire_at = copies_expire_at(row, holding);
        Some(dto)
    }

    fn environment(
        &self,
        environment: &AppEnvironment,
        row: Option<&app_environments::Model>,
        build: Option<Uuid>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> EnvironmentDto {
        let branch = branch_fields(environment, row);
        let found = build.and_then(|id| self.builds.get(&id).map(|b| (id, b)));
        EnvironmentDto {
            name: environment.name(),
            kind: environment.kind().as_str().to_string(),
            status: "active".to_string(),
            build_id: found.map(|(_, (label, _))| label.clone()),
            build_uuid: found.map(|(id, _)| id),
            semantic_revision_id: found.and_then(|(_, (_, pin))| *pin),
            owner: None,
            created_at,
            updated_at,
            last_activity_at: None,
            expires_at: None,
            url: environment_url_for(environment, self.org_slug, &self.app.slug),
            oltp_schema: None,
            oltp_branch: None,
            copies_expire_at: None,
            parent: branch.parent,
            fork_at: branch.fork_at,
            planes: branch.planes,
            planes_error: branch.planes_error,
            planes_set: branch.planes_set,
        }
    }
}

/// What an environment reports of where it was cut from and how each plane
/// behaves.
#[derive(Debug, PartialEq, Eq)]
struct BranchFields {
    parent: Option<String>,
    fork_at: Option<DateTime<Utc>>,
    planes: Option<BTreeMap<String, String>>,
    planes_error: Option<String>,
    /// The row's own `planes` object as stored; `None` for production and a
    /// fixed environment with no row.
    planes_set: Option<serde_json::Value>,
}

/// The branch facts of `environment` from its `row`, where it has one: the
/// recorded parent and fork point, else the defaults for its kind; and its
/// plane modes. A `planes` value that does not parse is reported, not hidden,
/// and does not stop the rest of the environment being shown. A fixed
/// environment with no row resolves the defaults and has no recorded creation,
/// so no fork point; production has no parent, fork point or planes.
fn branch_fields(
    environment: &AppEnvironment,
    row: Option<&app_environments::Model>,
) -> BranchFields {
    if *environment == AppEnvironment::Production {
        return BranchFields {
            parent: None,
            fork_at: None,
            planes: None,
            planes_error: None,
            planes_set: None,
        };
    }
    let parent = row
        .and_then(|row| row.parent_name.clone())
        .or_else(|| default_parent(environment).map(|parent| parent.name()));
    let fork_at = row.map(|row| row.fork_at.unwrap_or(row.created_at).with_timezone(&Utc));
    let spec = match row {
        Some(row) => PlaneSpec::parse(&row.planes),
        None => Ok(PlaneSpec::default()),
    };
    let (planes, planes_error) = match spec {
        Ok(spec) => (
            spec.resolve(environment).map(|resolved| {
                resolved
                    .entries()
                    .into_iter()
                    .map(|(plane, mode)| (plane.key().to_string(), mode.to_string()))
                    .collect()
            }),
            None,
        ),
        Err(message) => (None, Some(message)),
    };
    BranchFields {
        parent,
        fork_at,
        planes,
        planes_error,
        planes_set: row.map(|row| row.planes.clone()),
    }
}

/// The cut of the org's active OLTP staging branch — looked up only when a
/// shown row records a schema, so an app whose sandboxes have none pays no
/// query.
async fn oltp_cut(
    db: &DatabaseConnection,
    app: &apps::Model,
    rows: &[app_environments::Model],
) -> Result<Option<oxy_oltp::branches::BranchCut>, DbErr> {
    if rows.iter().all(|row| row.oltp_schema.is_none()) {
        return Ok(None);
    }
    let (_, branch) =
        oxy_oltp::branches::find(db, app.org_id, oxy_oltp::OltpBranch::Staging).await?;
    Ok(branch
        .filter(|row| row.status == oxy_oltp::entity::branches::BranchStatus::Active)
        .map(|row| oxy_oltp::branches::BranchCut::of(&row)))
}

/// The app's environment branches, keyed by environment — looked up only when a
/// shown row is a sandbox whose planes ask for one, so an app with none pays no
/// query.
async fn oltp_branches(
    db: &DatabaseConnection,
    app: &apps::Model,
    rows: &[app_environments::Model],
) -> Result<HashMap<String, OltpBranchDto>, DbErr> {
    if !rows.iter().any(|row| wants_branch(row)) {
        return Ok(HashMap::new());
    }
    let queued = crate::custom_apps_env_branch::task::recuts_queued(db, app.id).await?;
    Ok(oxy_oltp::env_branches::list_for_app(db, app.id)
        .await?
        .iter()
        .map(|row| {
            let dto = OltpBranchDto {
                recut_queued: queued.contains(&row.env_name),
                ..OltpBranchDto::of(row)
            };
            (row.env_name.clone(), dto)
        })
        .collect())
}

/// Which shown sandboxes' overlays still hold a copied table: one query for
/// all of them.
async fn holding(
    db: &DatabaseConnection,
    rows: &[app_environments::Model],
) -> Result<HashSet<String>, DbErr> {
    let keys: Vec<String> = rows
        .iter()
        .filter(|row| is_sandbox(row))
        .filter_map(overlay_key_of)
        .collect();
    holding_copies(db, &keys).await
}

/// Whether `row` is a sandbox and asks for a branch of its own.
fn wants_branch(row: &app_environments::Model) -> bool {
    is_sandbox(row) && crate::custom_apps_env_branch::wants_branch(row).is_some()
}

/// Whether `row` is a sandbox's.
pub(super) fn is_sandbox(row: &app_environments::Model) -> bool {
    row.kind == AppEnvironmentKind::Dev.as_str()
}

async fn load_builds(
    db: &DatabaseConnection,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, (String, Option<Uuid>)>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    // Three columns, not the row: `manifest_json` is the whole manifest.
    let rows: Vec<(Uuid, String, Option<Uuid>)> = app_builds::Entity::find()
        .select_only()
        .column(app_builds::Column::Id)
        .column(app_builds::Column::BuildId)
        .column(app_builds::Column::SemanticRevisionId)
        .filter(app_builds::Column::Id.is_in(ids))
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, label, pin)| (id, (label, pin)))
        .collect())
}

async fn load_emails(
    db: &DatabaseConnection,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, Option<String>>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Option<String>)> = users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .column(users::Column::Email)
        .filter(users::Column::Id.is_in(ids))
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
