//! `PUT …/{app}/environments/{env}/events`: a non-production environment's
//! events switch (`app_environments.events_enabled`, design §6.7). Off, the
//! environment's listeners hold what is published to them; on, they receive
//! it. Production always delivers and has no switch. Audited.

use entity::{app_environments, apps};
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use serde_json::json;

use super::reply::{ApiError, environment};

/// The audit action of a flip.
pub const SWITCHED: &str = "app.environment.events";

#[derive(Debug, Serialize)]
pub(super) struct SwitchReply {
    environment: String,
    enabled: bool,
}

/// Set `app`'s environment `name`'s switch to `enabled`.
pub(super) async fn set(
    db: &DatabaseConnection,
    actor: &RequestActor,
    app: &apps::Model,
    name: &str,
    enabled: bool,
) -> Result<SwitchReply, ApiError> {
    let environment = switchable(name)?;
    let name = environment.name();
    let flipped = app_environments::Entity::update_many()
        .col_expr(
            app_environments::Column::EventsEnabled,
            Expr::value(enabled),
        )
        .filter(app_environments::Column::AppId.eq(app.id))
        .filter(app_environments::Column::Name.eq(name.clone()))
        .filter(app_environments::Column::DeletingAt.is_null())
        .exec(db)
        .await
        .map_err(|e| ApiError::db("set the events switch", e))?;
    if flipped.rows_affected != 1 {
        return Err(missing(db, app, &name).await);
    }
    let entry = AuditEntry::for_request(actor, SWITCHED)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{name}", app.id),
            format!("{}/{name}", app.slug),
        )
        .environment(name.clone())
        .metadata(json!({ "enabled": enabled }));
    audit::record_best_effort(db, entry).await;
    Ok(SwitchReply {
        environment: name,
        enabled,
    })
}

/// A non-production environment's name; production has no switch.
fn switchable(name: &str) -> Result<AppEnvironment, ApiError> {
    match environment(Some(name))? {
        AppEnvironment::Production => Err(ApiError::Conflict(
            "production_has_no_events_switch",
            "production has no events switch: its events are always delivered".into(),
        )),
        environment => Ok(environment),
    }
}

/// Why no row was flipped: there is none, or it is being deleted.
async fn missing(db: &DatabaseConnection, app: &apps::Model, name: &str) -> ApiError {
    match app_environments::Entity::find_by_id((app.id, name.to_string()))
        .one(db)
        .await
    {
        Ok(Some(_)) => {
            ApiError::Conflict("environment_deleting", format!("`{name}` is being deleted"))
        }
        Ok(None) => ApiError::NotFound(
            "environment_not_found",
            format!("`{}` has no environment `{name}`", app.slug),
        ),
        Err(e) => ApiError::db("read the environment", e),
    }
}
