//! Step 2 of a batch: the writer's app, and its workspace's Airhouse. A
//! writer without either is held for that, and ships nothing.

use entity::apps;
use oxy_oltp::schema::{WriterRef, app_writer_name};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::{Pass, Stop, hold};

/// A writer with somewhere to ship: its app's workspace has an Airhouse.
pub(super) struct Target {
    /// The bare writer name: `oltp_<name>` in Airhouse.
    pub name: String,
    /// `app_<name>`, the schema the host signs.
    pub schema: String,
    pub workspace: Uuid,
    /// Its held tables, which grow as the pass holds more.
    pub held: Vec<String>,
    /// Whether `oltp_changes_stale` was logged for it this pass.
    pub stale_logged: bool,
}

/// Step 2: the writer's app and its workspace Airhouse. `None`, with the
/// writer held, when the org has no such app or its workspace no active
/// Airhouse tenant. A lookup that cannot tell, as in a worker with no
/// Airhouse token broker, fails the writer for this pass instead: a
/// misconfigured worker must not hold every org's writers.
pub(super) async fn find(pass: &Pass<'_>, writer: &str) -> Result<Option<Target>, Stop> {
    let app = writer_app(pass.db, pass.org_id, writer)
        .await
        .map_err(|e| Stop::Failed(format!("find the writer's app: {e}")))?;
    let Some(app) = app else {
        hold(
            pass,
            writer,
            None,
            "no app of the org writes as this writer",
        )
        .await?;
        return Ok(None);
    };
    let workspace = app.project_id;
    let has_airhouse = pass
        .sink
        .has_airhouse(pass.db, workspace)
        .await
        .map_err(|e| Stop::Failed(format!("look up the app's workspace Airhouse: {e}")))?;
    if !has_airhouse {
        let reason = format!("app {} has no Airhouse in workspace {workspace}", app.slug);
        hold(pass, writer, None, &reason).await?;
        return Ok(None);
    }
    let schema = WriterRef::app(writer.to_owned())
        .map_err(|e| Stop::Failed(format!("the writer's schema: {e}")))?
        .schema_name();
    Ok(Some(Target {
        name: writer.to_owned(),
        schema,
        workspace,
        held: pass.holds.tables(writer),
        stale_logged: false,
    }))
}

/// The org's app whose writer is `writer`: `app_writer_name` turns a slug's
/// `-` into `_` and refuses a slug holding `_`, so its slug is the writer
/// name with each `_` turned back.
async fn writer_app(
    db: &DatabaseConnection,
    org_id: Uuid,
    writer: &str,
) -> Result<Option<apps::Model>, DbErr> {
    let slug = writer.replace('_', "-");
    if app_writer_name(&slug).as_deref() != Some(writer) {
        return Ok(None);
    }
    apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(slug))
        .one(db)
        .await
}
