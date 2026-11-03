//! Reading an environment's planes: the one rule for what its
//! `app_environments` row says, shared by admission
//! (`environment_gate::with_planes`) and by every caller that only needs the
//! modes (the agent ask and the automation run on a non-production host).

use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ConnectionTrait, EntityTrait};
use uuid::Uuid;

use super::{PlaneSpec, ResolvedPlanes};

/// The planes `environment` runs with, from its `row`: the row's choices over
/// the defaults for its kind. A staging environment with no row resolves the
/// defaults (a fixed environment may have none; a sandbox always has one).
/// Production has no planes, whatever its row says, and its row is not
/// parsed. A row whose `planes` do not parse is `Err` with the parse error:
/// the defaults are never a fallback for a choice nobody can read.
pub fn planes_from_row(
    row: Option<&app_environments::Model>,
    environment: &AppEnvironment,
) -> Result<Option<ResolvedPlanes>, String> {
    if *environment == AppEnvironment::Production {
        return Ok(None);
    }
    let spec = match row {
        Some(row) => PlaneSpec::parse(&row.planes)?,
        None => PlaneSpec::default(),
    };
    Ok(spec.resolve(environment))
}

/// [`planes_from_row`] for the row of `environment` of the app `app_id`,
/// read here. `Ok(None)` for production, which is not read. `Err` carries the
/// parse error, or `database: …` for a failed read.
pub async fn environment_planes(
    db: &impl ConnectionTrait,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<Option<ResolvedPlanes>, String> {
    if *environment == AppEnvironment::Production {
        return Ok(None);
    }
    let row = app_environments::Entity::find_by_id((app_id, environment.name()))
        .one(db)
        .await
        .map_err(|e| format!("database: {e}"))?;
    planes_from_row(row.as_ref(), environment)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;

    fn row(name: &str, planes: serde_json::Value) -> app_environments::Model {
        let now = Utc::now().fixed_offset();
        app_environments::Model {
            app_id: Uuid::from_u128(9),
            name: name.to_string(),
            kind: if name == "staging" { "staging" } else { "dev" }.to_string(),
            owner_user_id: None,
            build_id: None,
            updated_by: None,
            updated_at: now,
            created_at: now,
            deleting_at: None,
            oltp_schema: None,
            created_by_token_id: None,
            schedules_enabled: false,
            parent_name: None,
            fork_at: None,
            planes,
            semantic_revision_id: None,
            semantic_git_sha: None,
        }
    }

    fn sandbox() -> AppEnvironment {
        AppEnvironment::parse("dev-a1").expect("a sandbox")
    }

    #[test]
    fn staging_with_no_row_gets_the_defaults() {
        let planes = planes_from_row(None, &AppEnvironment::Staging)
            .expect("readable")
            .expect("staging has planes");
        assert_eq!(
            Some(planes),
            PlaneSpec::default().resolve(&AppEnvironment::Staging)
        );
    }

    #[test]
    fn a_rows_choice_is_read_over_the_defaults() {
        let held = row("dev-a1", json!({"workspace-actions": "held"}));
        let planes = planes_from_row(Some(&held), &sandbox())
            .expect("readable")
            .expect("a sandbox has planes");
        assert_eq!(
            planes.workspace_actions,
            super::super::WorkspaceActionsMode::Held
        );
    }

    #[test]
    fn production_is_none_and_its_row_is_not_parsed() {
        assert_eq!(planes_from_row(None, &AppEnvironment::Production), Ok(None));
        let unreadable = row("production", json!({"nope": "x"}));
        assert_eq!(
            planes_from_row(Some(&unreadable), &AppEnvironment::Production),
            Ok(None)
        );
    }

    #[test]
    fn an_unreadable_row_is_an_error_naming_the_fault_never_the_defaults() {
        let error = planes_from_row(
            Some(&row("staging", json!({"olpt": "schema"}))),
            &AppEnvironment::Staging,
        )
        .expect_err("unreadable");
        assert!(error.contains("olpt"), "{error}");
        planes_from_row(
            Some(&row("dev-a1", json!({"oltp": "teleport"}))),
            &sandbox(),
        )
        .expect_err("unreadable");
    }

    /// Production asks nothing: the connection is disconnected.
    #[tokio::test]
    async fn production_is_not_read() {
        let db = sea_orm::DatabaseConnection::default();
        let planes = environment_planes(&db, Uuid::from_u128(9), &AppEnvironment::Production).await;
        assert_eq!(planes, Ok(None));
    }
}
