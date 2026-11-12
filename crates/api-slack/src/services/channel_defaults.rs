use chrono::Utc;
use entity::prelude::SlackChannelDefaults;
use entity::slack_channel_defaults;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

pub struct ChannelDefaultsService;

pub struct UpsertChannelDefault {
    pub installation_id: Uuid,
    pub slack_channel_id: String,
    pub workspace_id: Uuid,
    pub set_by_user_link_id: Uuid,
}

impl ChannelDefaultsService {
    /// Fetch the channel's default workspace, if any.
    pub async fn get(
        installation_id: Uuid,
        slack_channel_id: &str,
    ) -> Result<Option<slack_channel_defaults::Model>, OxyError> {
        let conn = establish_connection().await?;
        SlackChannelDefaults::find()
            .filter(slack_channel_defaults::Column::InstallationId.eq(installation_id))
            .filter(slack_channel_defaults::Column::SlackChannelId.eq(slack_channel_id.to_string()))
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
    }

    /// Upsert (installation, channel) → workspace_id. Replaces any existing
    /// default. Atomic via Postgres `INSERT ... ON CONFLICT DO UPDATE` so two
    /// concurrent "Set as default" submissions on the same channel can't
    /// collide on the unique index and bubble up a raw `DbErr`.
    pub async fn upsert(
        input: UpsertChannelDefault,
    ) -> Result<slack_channel_defaults::Model, OxyError> {
        let conn = establish_connection().await?;
        let now = Utc::now();
        let model = slack_channel_defaults::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            installation_id: ActiveValue::Set(input.installation_id),
            slack_channel_id: ActiveValue::Set(input.slack_channel_id),
            workspace_id: ActiveValue::Set(input.workspace_id),
            set_by_user_link_id: ActiveValue::Set(Some(input.set_by_user_link_id)),
            created_at: ActiveValue::Set(now.into()),
            updated_at: ActiveValue::Set(now.into()),
        };
        SlackChannelDefaults::insert(model)
            .on_conflict(
                OnConflict::columns([
                    slack_channel_defaults::Column::InstallationId,
                    slack_channel_defaults::Column::SlackChannelId,
                ])
                .update_columns([
                    slack_channel_defaults::Column::WorkspaceId,
                    slack_channel_defaults::Column::SetByUserLinkId,
                    slack_channel_defaults::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec_with_returning(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
    }

    /// Clear the channel default.
    pub async fn clear(installation_id: Uuid, slack_channel_id: &str) -> Result<(), OxyError> {
        let conn = establish_connection().await?;
        use sea_orm::EntityTrait;
        SlackChannelDefaults::delete_many()
            .filter(slack_channel_defaults::Column::InstallationId.eq(installation_id))
            .filter(slack_channel_defaults::Column::SlackChannelId.eq(slack_channel_id.to_string()))
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(())
    }
}
