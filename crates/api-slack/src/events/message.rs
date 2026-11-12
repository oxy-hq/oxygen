//! Handler for Slack `message` events.
//!
//! Handles:
//! - DM messages: always runs if the user is linked; shows connect-account prompt otherwise.
//! - Channel thread replies: ignored. In channels, the only way to talk to Oxy is to
//!   @-mention it — even inside an existing Oxy-started thread, follow-ups that don't
//!   tag the bot are silent. This matches how Claude (and most well-behaved Slack
//!   assistants) operate; replying to a thread without an @-mention should never feel
//!   like the bot is eavesdropping.
//!
//! When a user *does* @-mention Oxy in a thread reply, Slack delivers a separate
//! `app_mention` event in addition to this `message` event — `app_mention` is the
//! sole entry point for channel responses, and it correctly reuses the existing
//! Oxy thread context via `run_or_prompt`. So suppressing this handler in channels
//! is both the right UX and the right way to avoid double-replies.

use crate::error::SlackError;
use crate::events::execution::{SlackRunRequest, run_for_slack};
use crate::linking::magic_link::new_link_url;
use crate::resolution::entrypoint::run_or_prompt;
use crate::resolution::thread_context::ThreadContextService;
use crate::resolution::user::{ResolvedUser, resolve};
use entity::slack_installations::Model as InstallationRow;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;

pub struct MessageArgs {
    pub installation: InstallationRow,
    pub bot_token: String,
    pub user: Option<String>,
    pub text: Option<String>,
    pub ts: String,
    pub channel: String,
    pub thread_ts: Option<String>,
    pub channel_type: Option<String>,
    pub subtype: Option<String>,
    pub bot_id: Option<String>,
    /// The node's Layer-1 pre-aggregation cache, threaded down to the agent
    /// run. A default carries no cache, which only costs the run a rollup
    /// read — the semantic queries compile to warehouse SQL instead.
    pub preagg: PreaggCacheCtx,
}

pub async fn handle(args: MessageArgs) -> Result<(), SlackError> {
    // Ignore bot messages.
    if args.bot_id.is_some() {
        return Ok(());
    }
    if matches!(
        args.subtype.as_deref(),
        Some("bot_message" | "message_changed")
    ) {
        return Ok(());
    }

    let (Some(user), Some(text)) = (args.user, args.text) else {
        return Ok(());
    };

    let is_dm = args.channel_type.as_deref() == Some("im");

    if is_dm {
        return handle_dm(
            args.installation,
            args.bot_token,
            user,
            text,
            args.ts,
            args.channel,
            args.thread_ts,
            args.preagg,
        )
        .await;
    }

    // Channel messages are only handled via the `app_mention` event — see
    // module docs for the reasoning. Drop everything else silently.
    Ok(())
}

async fn handle_dm(
    installation: InstallationRow,
    bot_token: String,
    user: String,
    text: String,
    ts: String,
    channel: String,
    thread_ts_opt: Option<String>,
    preagg: PreaggCacheCtx,
) -> Result<(), SlackError> {
    let thread_ts = thread_ts_opt.unwrap_or_else(|| ts.clone());

    let link = match resolve(&installation, &user).await? {
        ResolvedUser::Linked(l) => l,
        ResolvedUser::Unlinked => {
            let connect_url = new_link_url(
                &installation.slack_team_id,
                &user,
                Some(&channel),
                Some(&thread_ts),
            )
            .await?;
            return Err(SlackError::NotAuthenticated { connect_url });
        }
    };

    // Check for existing thread context (e.g. follow-up in same DM conversation).
    if let Some(ctx) = ThreadContextService::find(installation.id, &channel, &thread_ts).await? {
        return run_for_slack(SlackRunRequest {
            installation,
            bot_token,
            user_link: link,
            workspace_id: ctx.workspace_id,
            agent_path: ctx.agent_path,
            question: text,
            channel_id: channel,
            thread_ts,
            preagg,
        })
        .await;
    }

    run_or_prompt(
        installation,
        bot_token,
        link,
        text,
        channel,
        thread_ts,
        true,
        preagg,
    )
    .await
}
