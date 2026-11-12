use entity::prelude::SlackThreads;
use entity::slack_threads;
use oxy::database::client::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::prelude::Expr;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

pub struct ThreadContextService;

pub struct CreateThreadContext {
    pub installation_id: Uuid,
    pub slack_channel_id: String,
    pub slack_thread_ts: String,
    pub workspace_id: Uuid,
    pub agent_path: String,
    pub oxy_thread_id: Uuid,
    pub initiated_by_user_link_id: Option<Uuid>,
}

impl ThreadContextService {
    pub async fn find(
        installation_id: Uuid,
        channel: &str,
        thread_ts: &str,
    ) -> Result<Option<slack_threads::Model>, OxyError> {
        let conn = establish_connection().await?;
        SlackThreads::find()
            .filter(slack_threads::Column::InstallationId.eq(installation_id))
            .filter(slack_threads::Column::SlackChannelId.eq(channel))
            .filter(slack_threads::Column::SlackThreadTs.eq(thread_ts))
            .one(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))
    }

    pub async fn create(input: CreateThreadContext) -> Result<slack_threads::Model, OxyError> {
        let conn = establish_connection().await?;
        slack_threads::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            installation_id: ActiveValue::Set(input.installation_id),
            slack_channel_id: ActiveValue::Set(input.slack_channel_id),
            slack_thread_ts: ActiveValue::Set(input.slack_thread_ts),
            workspace_id: ActiveValue::Set(input.workspace_id),
            agent_path: ActiveValue::Set(input.agent_path),
            oxy_thread_id: ActiveValue::Set(input.oxy_thread_id),
            initiated_by_user_link_id: ActiveValue::Set(input.initiated_by_user_link_id),
            last_slack_message_ts: ActiveValue::Set(None),
            created_at: ActiveValue::NotSet,
            updated_at: ActiveValue::NotSet,
        }
        .insert(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))
    }

    pub async fn update_last_ts(id: Uuid, ts: &str) -> Result<(), OxyError> {
        let conn = establish_connection().await?;
        SlackThreads::update_many()
            .col_expr(
                slack_threads::Column::LastSlackMessageTs,
                Expr::value(ts.to_string()),
            )
            .filter(slack_threads::Column::Id.eq(id))
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(())
    }

    /// Overwrite the workspace and agent stored for a thread. Called when the
    /// user switches workspaces via "Wrong workspace?" so follow-up messages
    /// use the newly chosen workspace rather than the original one.
    pub async fn update_workspace(
        id: Uuid,
        workspace_id: Uuid,
        agent_path: &str,
    ) -> Result<(), OxyError> {
        let conn = establish_connection().await?;
        SlackThreads::update_many()
            .col_expr(
                slack_threads::Column::WorkspaceId,
                Expr::value(workspace_id),
            )
            .col_expr(
                slack_threads::Column::AgentPath,
                Expr::value(agent_path.to_string()),
            )
            .filter(slack_threads::Column::Id.eq(id))
            .exec(&conn)
            .await
            .map_err(|e| OxyError::DBError(e.to_string()))?;
        Ok(())
    }
}
