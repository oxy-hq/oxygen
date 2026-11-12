use entity::prelude::SlackUserPreferences;
use entity::slack_user_preferences;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

pub struct UserPreferencesService;

pub struct UpsertPrefs {
    pub user_link_id: Uuid,
    pub default_workspace_id: Option<Uuid>,
    pub default_agent_path: Option<String>,
}

impl UserPreferencesService {
    pub async fn get(
        user_link_id: Uuid,
    ) -> Result<Option<slack_user_preferences::Model>, OxyError> {
        let conn = establish_connection().await?;
        SlackUserPreferences::find()
            .filter(slack_user_preferences::Column::UserLinkId.eq(user_link_id))
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
    }

    pub async fn upsert(input: UpsertPrefs) -> Result<slack_user_preferences::Model, OxyError> {
        if let Some(existing) = Self::get(input.user_link_id).await? {
            let conn = establish_connection().await?;
            let mut active: slack_user_preferences::ActiveModel = existing.into();
            active.default_workspace_id = ActiveValue::Set(input.default_workspace_id);
            active.default_agent_path = ActiveValue::Set(input.default_agent_path);
            active.updated_at = ActiveValue::NotSet;
            active
                .update(&conn)
                .await
                .map_err(|e| OxyError::DBError(e.to_string()))
        } else {
            let conn = establish_connection().await?;
            slack_user_preferences::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                user_link_id: ActiveValue::Set(input.user_link_id),
                default_workspace_id: ActiveValue::Set(input.default_workspace_id),
                default_agent_path: ActiveValue::Set(input.default_agent_path),
                updated_at: ActiveValue::NotSet,
            }
            .insert(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
        }
    }
}
