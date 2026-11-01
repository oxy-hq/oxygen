//! Who is asking for the home, and in which org: the database, the caller's
//! facts, the org they belong to, and the org teams they are in.

use axum::http::StatusCode;
use oxy::database::client::establish_connection;
use oxy_app_core::widgets::layout::Viewer;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::PrincipalFacts;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement};
use uuid::Uuid;

use super::load;

/// A caller and the org they named.
pub struct Asker {
    pub db: DatabaseConnection,
    pub facts: PrincipalFacts,
    /// The org with the slug they named, when they belong to it. An org they do
    /// not belong to is `None`, however wide their staff reach.
    pub org_id: Option<Uuid>,
}

/// Resolve `org_slug` for `user`. Unknown facts answer 503, never an empty
/// answer: "nothing here" on a blip would be a lie.
pub async fn asker(user: &AuthenticatedUser, org_slug: &str) -> Result<Asker, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let caller = oxy_server_authz::Caller::from_user(user);
    let facts = oxy_server_authz::loader::load_principal_facts_scoped(&db, &caller, false)
        .await
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let org_id = load::member_org_id(&db, &facts, org_slug)
        .await
        .map_err(|e| {
            tracing::error!("home: org lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Asker { db, facts, org_id })
}

/// The caller as a layout's `for` sees them: their email and the names of the
/// org teams they belong to in `org_id`.
pub async fn viewer(
    db: &DatabaseConnection,
    user_id: Uuid,
    email: Option<&str>,
    org_id: Uuid,
) -> Result<Viewer, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT t.name FROM org_teams t \
             JOIN org_team_members m ON m.team_id = t.id \
             WHERE t.org_id = $1 AND m.user_id = $2",
            [org_id.into(), user_id.into()],
        ))
        .await?;
    let teams = rows
        .iter()
        .map(|r| r.try_get::<String>("", "name"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Viewer::new(email, teams))
}
