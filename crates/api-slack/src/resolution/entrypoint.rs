//! Shared "resolve and run or prompt for picker" path used by both
//! app_mention and DM message handlers.

use base64::Engine;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::client::SlackClient;
use crate::error::SlackError;
use crate::events::execution::{SlackRunRequest, run_for_slack};
use crate::pickers::workspace::workspace_picker_blocks;
use crate::resolution::thread_context::ThreadContextService;
use crate::resolution::workspace_agent::{Resolution, resolve as resolve_ws};
use entity::slack_installations::Model as InstallationRow;
use entity::slack_user_links::Model as UserLinkRow;
use oxy::database::client::establish_connection;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_shared::errors::OxyError;

/// Core dispatch logic: check for existing thread context, resolve workspace/agent,
/// then either run the agent immediately or post an ephemeral explaining the issue.
pub async fn run_or_prompt(
    installation: InstallationRow,
    bot_token: String,
    user_link: UserLinkRow,
    question: String,
    channel_id: String,
    thread_ts: String,
    is_dm: bool,
    // The node's Layer-1 pre-aggregation cache, handed to whichever run this
    // dispatches. A default means no rollup: the run's semantic queries compile
    // to warehouse SQL, a correct answer that is merely slower.
    preagg: PreaggCacheCtx,
) -> Result<(), SlackError> {
    let client = SlackClient::new();

    // 0. Guard: user must still be a member of the installation's org.
    if !is_org_member(installation.org_id, user_link.oxy_user_id).await? {
        return Err(SlackError::NotOrgMember);
    }

    // 1. Existing thread context? Reuse it directly.
    if let Some(ctx) = ThreadContextService::find(installation.id, &channel_id, &thread_ts).await? {
        return run_for_slack(SlackRunRequest {
            installation,
            bot_token,
            user_link,
            workspace_id: ctx.workspace_id,
            agent_path: ctx.agent_path,
            question,
            channel_id,
            thread_ts,
            preagg,
        })
        .await;
    }

    // 2. Auto-resolve workspace/agent — may show a picker if ambiguous.
    match resolve_ws(&installation, &user_link, &channel_id).await? {
        Resolution::NoWorkspaces(reason) => {
            tracing::warn!(
                oxy_user_id = %user_link.oxy_user_id,
                org_id = %installation.org_id,
                slack_user_id = user_link.slack_user_id,
                reason = ?reason,
                "slack resolve: NoWorkspaces"
            );
            return Err(SlackError::NoWorkspaces(reason));
        }
        Resolution::WorkspaceHasNoAgents { workspace_name, .. } => {
            tracing::warn!(
                org_id = %installation.org_id,
                workspace_name,
                "slack resolve: workspace has no agents"
            );
            return Err(SlackError::NoAgentsInWorkspace { workspace_name });
        }
        Resolution::Resolved {
            workspace_id,
            agent_path,
        } => {
            run_for_slack(SlackRunRequest {
                installation,
                bot_token,
                user_link,
                workspace_id,
                agent_path,
                question,
                channel_id,
                thread_ts,
                preagg,
            })
            .await?;
        }
        Resolution::PickerNeeded { workspaces } => {
            tracing::info!(
                workspace_count = workspaces.len(),
                channel_id,
                is_dm,
                "slack resolve: multiple workspaces — showing picker"
            );

            let encoded_q = base64::engine::general_purpose::STANDARD.encode(question.as_bytes());
            let blocks = workspace_picker_blocks(&workspaces, &encoded_q);

            if is_dm {
                // In DM channels chat.postEphemeral is not delivered. Post the
                // picker directly into the thread — the conversation is already
                // private so there is no need to hide it from other users.
                client
                    .chat_post_message_with_blocks(
                        &bot_token,
                        &channel_id,
                        "Pick a workspace to run your query:",
                        Some(&thread_ts),
                        Some(blocks),
                    )
                    .await?;
            } else {
                // In channels: acknowledge publicly then send an ephemeral picker
                // so only the asker sees the workspace selection UI.
                client
                    .chat_post_message(
                        &bot_token,
                        &channel_id,
                        "I have access to a few workspaces — pick one below to run your query.",
                        Some(&thread_ts),
                    )
                    .await?;

                client
                    .chat_post_ephemeral(
                        &bot_token,
                        &channel_id,
                        &user_link.slack_user_id,
                        blocks,
                        "Pick a workspace",
                        Some(&thread_ts),
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

/// Returns true if the Oxy user is currently a member of the given org.
async fn is_org_member(org_id: uuid::Uuid, user_id: uuid::Uuid) -> Result<bool, OxyError> {
    let conn = establish_connection().await?;
    let member = entity::org_members::Entity::find()
        .filter(entity::org_members::Column::OrgId.eq(org_id))
        .filter(entity::org_members::Column::UserId.eq(user_id))
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    Ok(member.is_some())
}
