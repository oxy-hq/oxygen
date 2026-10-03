//! Turning environment rows into the [`EnvironmentDto`] the API returns: one
//! query each for the builds, the owners' emails and the sandboxes' last
//! invocations, however many environments are shown.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{app_builds, app_environments, apps, users};
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use oxy_app_core::custom_apps_host_dispatch::environment_url_for;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use super::{EnvironmentDto, OwnerDto, activity, oltp_state};

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
        Some(dto)
    }

    fn environment(
        &self,
        environment: &AppEnvironment,
        build: Option<Uuid>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> EnvironmentDto {
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
        }
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
