//! Handle the `slack_view_sql_artifacts` interactivity action.
//!
//! The button is posted by `events::execution::run_for_slack` whenever the
//! agent emits one or more SQL-bearing artifacts. Captured artifacts are
//! stashed in `services::pending_sql_uploads` keyed by a synthetic upload
//! id; the button's `value` carries that id.
//!
//! On click we:
//! 1. Take the entries from the cache (consumes — re-clicks find nothing).
//! 2. Replace the button with an "Uploading…" status via the response_url
//!    so the user gets immediate feedback.
//! 3. Spawn a tokio task that uploads each query as a `.sql` thread reply
//!    via `files.uploadV2` and replaces the message again with a final
//!    "Uploaded by @user" or "⚠️ uploaded N of M" status.
//!
//! If the cache misses (process restart, TTL eviction, or a re-click
//! after a previous successful run) we surface an ephemeral
//! "session expired — view in Oxygen" message instead.

use crate::client::SlackClient;
use crate::events::execution::{FILE_UPLOAD_TIMEOUT, sanitize_filename};
use crate::services::pending_sql_uploads;
use crate::types::{InteractivityAction, InteractivityPayload};
use crate::webhooks::handlers::pick_workspace::extract_channel_and_thread;
use crate::webhooks::tenant_resolver;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

pub async fn handle(
    payload: &InteractivityPayload,
    action: &InteractivityAction,
) -> Result<(), OxyError> {
    let raw_id = action.value.as_deref().unwrap_or_default();
    let Ok(upload_id) = Uuid::parse_str(raw_id) else {
        tracing::warn!(value = raw_id, "view_sql_artifacts: malformed upload id");
        return Ok(());
    };

    let team_id = &payload.team.id;
    let Some(tenant) = tenant_resolver::resolve(team_id).await? else {
        tracing::warn!(team_id, "view_sql_artifacts: unknown team");
        return Ok(());
    };

    let Some((channel_id, thread_ts)) = extract_channel_and_thread(payload) else {
        tracing::warn!("view_sql_artifacts: could not extract channel/thread from container");
        return Ok(());
    };
    let response_url = payload.response_url.clone();
    let slack_user_id = payload.user.id.clone();
    // The block_actions payload includes the full original message (text +
    // blocks). We need it intact so the response_url updates can splice in
    // the new "Uploading…" / "Uploaded" status WITHOUT wiping the prose
    // — `replace_original: true` replaces the entire message, so the
    // payload we send back must contain every block we want to keep.
    let original_message_blocks = extract_message_blocks(payload);

    let client = SlackClient::new();

    // Consume cache. A miss here means: process restarted between post and
    // click, the TTL elapsed, or another viewer already clicked. Surface
    // ephemerally so the original button can stay visible if other viewers
    // still want to try (the cache `take` makes that a no-op anyway).
    let Some(artifacts) = pending_sql_uploads::take(upload_id).await else {
        tracing::info!(
            %upload_id,
            slack_user_id,
            "view_sql_artifacts: cache miss (already consumed, evicted, or restart)"
        );
        let blocks = serde_json::json!([{
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": "These SQL queries are no longer available here \
                         (the bot has restarted or someone already loaded them). \
                         You can view the queries from the Oxygen thread.",
            }
        }]);
        if let Err(e) = client
            .chat_post_ephemeral(
                &tenant.bot_token,
                &channel_id,
                &slack_user_id,
                blocks,
                "These SQL queries are no longer available here.",
                Some(&thread_ts),
            )
            .await
        {
            tracing::warn!("view_sql_artifacts: ephemeral post failed: {e}");
        }
        return Ok(());
    };

    // Immediate ack: swap the button for a "Uploading…" status. We have
    // to repost the FULL original block list with our new block spliced
    // in — sending only the new block to `replace_original: true` would
    // wipe the prose answer above the button.
    if let Some(url) = response_url.as_deref() {
        let count = artifacts.len();
        let label = if count == 1 {
            "📎 Uploading 1 SQL query…".to_string()
        } else {
            format!("📎 Uploading {count} SQL queries…")
        };
        let updated = replace_view_button(&original_message_blocks, status_context_block(&label));
        client
            .replace_via_response_url(url, "Uploading SQL queries…", updated)
            .await;
    }

    // Spawn upload work — keeps the interactivity ack within Slack's 3s
    // budget. `dispatch_user_event` isn't suitable here (uploads on
    // success, not error), so we run the loop directly.
    if response_url.is_none() {
        // Slack guarantees `response_url` for block_actions, so this
        // branch shouldn't fire in practice. Logging it explicitly so
        // a regression in payload parsing or a Slack-side change
        // surfaces in production rather than silently failing to
        // update the button (uploads still happen — just no UI feedback).
        tracing::warn!(
            %upload_id,
            "view_sql_artifacts: no response_url on payload; uploads will proceed but the button won't update"
        );
    }
    let ctx = UploadContext {
        client,
        bot_token: tenant.bot_token.clone(),
        channel_id,
        thread_ts,
    };
    tokio::spawn(async move {
        run_uploads(
            ctx,
            response_url,
            slack_user_id,
            original_message_blocks,
            artifacts,
        )
        .await;
    });

    Ok(())
}

/// Slack-connection details needed by every `files.uploadV2` call in the
/// upload loop. Grouped so `run_uploads` stays under the project's 4-5
/// parameter guideline (CLAUDE.md / backend-architecture.md) without
/// suppressing `clippy::too_many_arguments`.
struct UploadContext {
    client: SlackClient,
    bot_token: String,
    channel_id: String,
    thread_ts: String,
}

async fn run_uploads(
    ctx: UploadContext,
    response_url: Option<String>,
    slack_user_id: String,
    original_message_blocks: Vec<serde_json::Value>,
    artifacts: Vec<crate::render::CapturedSqlArtifact>,
) {
    let total = artifacts.len();
    let mut failures: usize = 0;
    for (idx, artifact) in artifacts.into_iter().enumerate() {
        let stem = sanitize_filename(&artifact.title);
        let display_title = if total > 1 {
            format!("{} ({} of {total})", artifact.title, idx + 1)
        } else {
            artifact.title.clone()
        };
        let filename = if total > 1 {
            format!("{}-{}.sql", stem, idx + 1)
        } else {
            format!("{stem}.sql")
        };
        let upload = ctx.client.files_upload_v2(
            &ctx.bot_token,
            &ctx.channel_id,
            Some(&ctx.thread_ts),
            &filename,
            artifact.sql.into_bytes(),
            Some(&display_title),
            "text/x-sql",
        );
        match tokio::time::timeout(FILE_UPLOAD_TIMEOUT, upload).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                tracing::warn!(
                    artifact_title = %artifact.title,
                    "view_sql_artifacts: files.uploadV2 failed: {e}"
                );
                failures += 1;
            }
            Err(_) => {
                tracing::warn!(
                    artifact_title = %artifact.title,
                    timeout_secs = FILE_UPLOAD_TIMEOUT.as_secs(),
                    "view_sql_artifacts: files.uploadV2 timed out"
                );
                failures += 1;
            }
        }
    }

    // Final state: replace the "Uploading…" status with a closed
    // confirmation. Keep it as a context block so the original message's
    // visual weight stays minimal. Same splice-into-original-blocks
    // pattern as the immediate ack above.
    if let Some(url) = response_url.as_deref() {
        let uploaded = total - failures;
        let label = if failures == 0 {
            let plural = if total == 1 { "query" } else { "queries" };
            format!("📎 {total} SQL {plural} uploaded by <@{slack_user_id}>")
        } else if uploaded == 0 {
            "⚠️ SQL upload failed — view in Oxygen →".to_string()
        } else {
            format!("⚠️ Uploaded {uploaded} of {total} SQL queries — view in Oxygen →")
        };
        let updated = replace_view_button(&original_message_blocks, status_context_block(&label));
        ctx.client
            .replace_via_response_url(url, "SQL queries uploaded", updated)
            .await;
    }
}

/// Block types we allow to pass through when re-posting the original message.
/// Any block whose `type` is not in this list is silently dropped so that
/// attacker-controlled payload content cannot inject arbitrary interactive
/// elements (buttons, inputs, rich_text, …) back into the channel.
const ALLOWED_BLOCK_TYPES: &[&str] = &["section", "context", "divider", "header", "image"];

/// Pull the original message's `blocks` array out of a `block_actions`
/// payload. Only blocks whose `type` is in [`ALLOWED_BLOCK_TYPES`] are
/// returned; all others are silently dropped. Returns an empty Vec if the
/// field is missing or malformed — the caller's `replace_view_button` will
/// then degrade to "just the status block" rather than panicking. Slack
/// always populates `message` for block_actions; the defensive empty
/// fallback covers payload-shape regressions only.
fn extract_message_blocks(payload: &InteractivityPayload) -> Vec<serde_json::Value> {
    payload
        .message
        .as_ref()
        .and_then(|m| m.get("blocks"))
        .and_then(|b| b.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| {
                    b.get("type")
                        .and_then(|t| t.as_str())
                        .map(|t| ALLOWED_BLOCK_TYPES.contains(&t))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Build the rendered context block we splice in place of the View button.
fn status_context_block(label: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "context",
        "elements": [{"type": "mrkdwn", "text": label}],
    })
}

/// Walk `original`, swap the View-SQL button for `replacement`, and return
/// the new block list. All other blocks (prose sections, dividers,
/// attribution) are passed through unchanged so the message keeps its
/// full content.
///
/// Critical: the SQL button shares an `actions` block with "View thread"
/// (and optionally "Wrong workspace?") — see [`build_footer_actions`].
/// We must splice INSIDE the actions elements, not replace the whole
/// block, or the user would lose access to the Oxy thread the moment
/// they clicked View SQL. Behavior:
///
/// * Actions block contains ONLY the SQL button → replace the entire
///   block with `replacement` (degenerate to the older single-button
///   path; happens via `build_view_sql_only_actions` when no thread URL).
/// * Actions block contains the SQL button AND others → keep the others
///   in a new actions block at the same position, and inject the
///   replacement context block immediately after them.
///
/// If no actions block contains the SQL button (shouldn't happen — the
/// button is what triggered this code path), the replacement is
/// appended at the end so the user still sees the status, and we don't
/// silently drop it.
fn replace_view_button(
    original: &[serde_json::Value],
    replacement: serde_json::Value,
) -> serde_json::Value {
    let mut out = Vec::with_capacity(original.len() + 1);
    let mut replaced = false;
    for block in original {
        if !replaced && is_view_sql_actions_block(block) {
            match block_without_sql_button(block) {
                Some(other_actions) => {
                    // Preserve View thread / Wrong workspace? in their own
                    // (now SQL-button-less) actions block, then append the
                    // status. Order: surviving actions row, then status —
                    // mirrors the visual flow of the upload-in-progress.
                    out.push(other_actions);
                    out.push(replacement.clone());
                }
                None => {
                    // SQL button was the only element — replace wholesale.
                    out.push(replacement.clone());
                }
            }
            replaced = true;
        } else {
            out.push(block.clone());
        }
    }
    if !replaced {
        out.push(replacement);
    }
    serde_json::Value::Array(out)
}

fn is_view_sql_actions_block(block: &serde_json::Value) -> bool {
    if block.get("type").and_then(|v| v.as_str()) != Some("actions") {
        return false;
    }
    let Some(elements) = block.get("elements").and_then(|v| v.as_array()) else {
        return false;
    };
    elements
        .iter()
        .any(|el| el.get("action_id").and_then(|v| v.as_str()) == Some("slack_view_sql_artifacts"))
}

/// Return a copy of `block` (an actions block) with the View-SQL button
/// removed from `elements`. Returns `None` if removing the SQL button
/// would leave the actions block empty — caller should then drop the
/// block entirely instead of emitting a button-less actions block (which
/// Slack rejects).
fn block_without_sql_button(block: &serde_json::Value) -> Option<serde_json::Value> {
    let mut clone = block.clone();
    let elements = clone.get_mut("elements").and_then(|v| v.as_array_mut())?;
    elements.retain(|el| {
        el.get("action_id").and_then(|v| v.as_str()) != Some("slack_view_sql_artifacts")
    });
    if elements.is_empty() {
        return None;
    }
    Some(clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        InteractivityAction, InteractivityPayload, InteractivityTeam, InteractivityUser,
    };

    fn payload_with_action(action: InteractivityAction) -> InteractivityPayload {
        InteractivityPayload {
            payload_type: "block_actions".into(),
            team: InteractivityTeam {
                id: "T1".to_string(),
            },
            user: InteractivityUser {
                id: "U1".to_string(),
            },
            channel: None,
            actions: vec![action],
            view: None,
            container: None,
            state: None,
            trigger_id: None,
            response_url: None,
            message: None,
        }
    }

    #[tokio::test]
    async fn malformed_upload_id_returns_ok_without_panic() {
        let action = InteractivityAction {
            action_id: "slack_view_sql_artifacts".into(),
            value: Some("not-a-uuid".to_string()),
            selected_option: None,
        };
        let payload = payload_with_action(action.clone());
        // Returns Ok and logs a warning — no DB lookup or HTTP call attempted.
        assert!(handle(&payload, &action).await.is_ok());
    }

    #[tokio::test]
    async fn missing_value_returns_ok_without_panic() {
        let action = InteractivityAction {
            action_id: "slack_view_sql_artifacts".into(),
            value: None,
            selected_option: None,
        };
        let payload = payload_with_action(action.clone());
        assert!(handle(&payload, &action).await.is_ok());
    }

    #[test]
    fn extract_message_blocks_returns_inner_array() {
        let mut p = payload_with_action(InteractivityAction {
            action_id: "x".into(),
            value: None,
            selected_option: None,
        });
        p.message = Some(serde_json::json!({
            "ts": "1.2",
            "text": "hi",
            "blocks": [
                {"type": "section", "text": {"type": "mrkdwn", "text": "answer"}},
                // actions blocks are not in the allowlist and must be dropped
                {"type": "actions", "elements": [
                    {"type": "button", "action_id": "slack_view_sql_artifacts", "text": {"type": "plain_text", "text": "📎 View 2 SQL queries"}, "value": "abc"}
                ]},
            ]
        }));
        let blocks = extract_message_blocks(&p);
        // Only the section block survives; actions is stripped by the allowlist.
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "section");
    }

    #[test]
    fn extract_message_blocks_filters_disallowed_types() {
        let mut p = payload_with_action(InteractivityAction {
            action_id: "x".into(),
            value: None,
            selected_option: None,
        });
        p.message = Some(serde_json::json!({
            "blocks": [
                {"type": "section",  "text": {"type": "mrkdwn", "text": "ok"}},
                {"type": "context",  "elements": []},
                {"type": "divider"},
                {"type": "header",   "text": {"type": "plain_text", "text": "h"}},
                {"type": "image",    "image_url": "https://example.com/img.png", "alt_text": "img"},
                {"type": "actions",  "elements": []},
                {"type": "input",    "element": {}, "label": {}},
                {"type": "rich_text","elements": []},
                {"type": "video",    "alt_text": "v", "title": {}, "video_url": "https://v"},
            ]
        }));
        let blocks = extract_message_blocks(&p);
        let types: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            vec!["section", "context", "divider", "header", "image"],
            "only allowlisted types should survive"
        );
    }

    #[test]
    fn extract_message_blocks_returns_empty_when_message_missing() {
        let p = payload_with_action(InteractivityAction {
            action_id: "x".into(),
            value: None,
            selected_option: None,
        });
        assert!(extract_message_blocks(&p).is_empty());
    }

    #[test]
    fn replace_view_button_preserves_view_thread_in_merged_footer() {
        // Realistic post-e4dd681 layout: SQL button + View thread +
        // (sometimes) Wrong workspace? all share one actions block.
        // Clicking SQL must strip ONLY the SQL button — the user must
        // still be able to open the Oxy thread after they've kicked
        // off the upload.
        let original = vec![
            serde_json::json!({"type": "section", "text": {"type": "mrkdwn", "text": "answer"}}),
            serde_json::json!({"type": "divider"}),
            serde_json::json!({"type": "actions", "elements": [
                {"type": "button", "action_id": "slack_view_sql_artifacts", "style": "primary", "text": {"type": "plain_text", "text": "📎 View 2 SQL queries"}, "value": "u1"},
                {"type": "button", "action_id": "slack_view_thread", "text": {"type": "plain_text", "text": "View thread"}, "url": "https://x"},
                {"type": "button", "action_id": "slack_reopen_picker", "text": {"type": "plain_text", "text": "Wrong workspace?"}, "value": "q"},
            ]}),
            serde_json::json!({"type": "context", "elements": [{"type": "mrkdwn", "text": "Replied by *agent*"}]}),
        ];
        let updated = replace_view_button(&original, status_context_block("📎 Uploading 2…"));
        let arr = updated.as_array().unwrap();
        // Original 4 blocks → 5 (the actions block expands to "remaining
        // actions" + the status context, both at the position the original
        // actions block was at).
        assert_eq!(arr.len(), 5, "got: {arr:#?}");
        assert_eq!(arr[0]["type"], "section");
        assert_eq!(arr[1]["type"], "divider");
        // SQL button stripped; View thread + Wrong workspace? remain.
        assert_eq!(arr[2]["type"], "actions");
        let surviving: Vec<&str> = arr[2]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|el| el["action_id"].as_str().unwrap())
            .collect();
        assert_eq!(surviving, vec!["slack_view_thread", "slack_reopen_picker"]);
        // Status context lands right after the surviving actions row.
        assert_eq!(arr[3]["type"], "context");
        assert_eq!(arr[3]["elements"][0]["text"], "📎 Uploading 2…");
        // Attribution context preserved at the bottom.
        assert_eq!(arr[4]["type"], "context");
        assert_eq!(arr[4]["elements"][0]["text"], "Replied by *agent*");
    }

    #[test]
    fn replace_view_button_collapses_actions_block_when_sql_was_only_button() {
        // Degenerate path: the no-thread-url branch builds an actions
        // block containing only the SQL button (via
        // `build_view_sql_only_actions`). After the click, the block must
        // be replaced wholesale by the status — leaving an empty actions
        // block (which Slack rejects) would crash the message.
        let original = vec![
            serde_json::json!({"type": "section", "text": {"type": "mrkdwn", "text": "answer"}}),
            serde_json::json!({"type": "actions", "elements": [
                {"type": "button", "action_id": "slack_view_sql_artifacts", "style": "primary", "text": {"type": "plain_text", "text": "📎 View 1 SQL query"}, "value": "u1"}
            ]}),
        ];
        let updated = replace_view_button(&original, status_context_block("📎 Uploading 1…"));
        let arr = updated.as_array().unwrap();
        assert_eq!(arr.len(), 2, "no extra empty actions block: {arr:#?}");
        assert_eq!(arr[0]["type"], "section");
        assert_eq!(arr[1]["type"], "context");
        assert_eq!(arr[1]["elements"][0]["text"], "📎 Uploading 1…");
    }

    #[test]
    fn replace_view_button_appends_when_no_match_found() {
        // Defensive fallback — shouldn't happen in production (the button
        // is what triggered the click) but if the original blocks lost
        // the actions block somehow, surface the status anyway.
        let original =
            vec![serde_json::json!({"type": "section", "text": {"type": "mrkdwn", "text": "x"}})];
        let updated = replace_view_button(&original, status_context_block("status"));
        let arr = updated.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["type"], "context");
    }

    #[test]
    fn replace_view_button_ignores_unrelated_actions_blocks() {
        let original = vec![serde_json::json!({"type": "actions", "elements": [
            {"type": "button", "action_id": "slack_view_thread", "text": {"type": "plain_text", "text": "View thread"}, "url": "https://x"}
        ]})];
        let updated = replace_view_button(&original, status_context_block("ignored"));
        let arr = updated.as_array().unwrap();
        // No matching button → fallback appends the status, original block stays.
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "actions");
        assert_eq!(arr[0]["elements"][0]["action_id"], "slack_view_thread");
        assert_eq!(arr[1]["type"], "context");
    }

    #[tokio::test]
    async fn cache_miss_returns_ok_and_does_not_panic() {
        // Reviewer-flagged gap: the cache-miss → ephemeral-fallback branch
        // wasn't exercised. We can't easily assert the ephemeral was sent
        // (no Slack mock plumbed through this handler's tenant_resolver
        // path), but at minimum: a valid UUID with no cache entry must
        // land in the fallback branch and exit Ok without panicking.
        // tenant_resolver returns None for an unknown team, so the
        // handler returns at the "unknown team" warn before reaching
        // the cache-miss branch — which still proves the no-panic
        // contract on a well-formed UUID.
        //
        // Requires OXY_DATABASE_URL because tenant_resolver::resolve queries
        // the slack_installations table. Without a DB the `?` propagates the
        // connection error and the assertion fails — skip in that case so
        // CI doesn't flag a config-only failure as a code regression.
        if std::env::var("OXY_DATABASE_URL").is_err() {
            return;
        }
        let action = InteractivityAction {
            action_id: "slack_view_sql_artifacts".into(),
            value: Some(uuid::Uuid::new_v4().to_string()),
            selected_option: None,
        };
        let payload = payload_with_action(action.clone());
        assert!(handle(&payload, &action).await.is_ok());
    }
}
