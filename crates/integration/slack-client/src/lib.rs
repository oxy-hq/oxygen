use oxy_shared::errors::OxyError;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

mod users;
pub use users::{EmailLookup, LookupUser};

const DEFAULT_BASE_URL: &str = "https://slack.com/api";

/// Thin wrapper around Slack's Web API. Stateless on bot tokens —
/// every call takes the token as an argument so the same client can
/// serve multiple tenants.
#[derive(Debug, Clone)]
pub struct SlackClient {
    http: Client,
    base_url: String,
}

impl Default for SlackClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SlackClient {
    /// ~15 webhook/event handlers construct a `SlackClient` per event, so the
    /// client is the process-wide one (`oxy_shared::http::client`): pooled
    /// keep-alive connections to slack.com, a timeout and a user agent.
    pub fn new() -> Self {
        Self {
            http: oxy_shared::http::client(),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// For tests: point at a mock server.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: oxy_shared::http::client(),
            base_url: base_url.into(),
        }
    }

    /// POST form-encoded body (most Slack endpoints).
    pub async fn post_form(
        &self,
        method: &str,
        bot_token: Option<&str>,
        form: &[(&str, &str)],
    ) -> Result<Value, OxyError> {
        let url = format!("{}/{method}", self.base_url);
        let mut req = self.http.post(&url).form(form);
        if let Some(t) = bot_token {
            req = req.bearer_auth(t);
        }
        let resp: Value = req
            .send()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("slack {method} http: {e}")))?
            .json()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("slack {method} json: {e}")))?;
        check_ok(method, &resp)?;
        Ok(resp)
    }

    /// POST JSON body (chat.postMessage, views.publish, etc.).
    pub async fn post_json(
        &self,
        method: &str,
        bot_token: &str,
        body: &Value,
    ) -> Result<Value, OxyError> {
        let url = format!("{}/{method}", self.base_url);
        let resp: Value = self
            .http
            .post(&url)
            .bearer_auth(bot_token)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(body)
            .send()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("slack {method} http: {e}")))?
            .json()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("slack {method} json: {e}")))?;
        check_ok(method, &resp)?;
        Ok(resp)
    }

    // ---- High-level helpers used by handlers ----

    pub async fn oauth_v2_access(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
    ) -> Result<OauthV2Access, OxyError> {
        let v = self
            .post_form(
                "oauth.v2.access",
                None,
                &[
                    ("client_id", client_id),
                    ("client_secret", client_secret),
                    ("code", code),
                    ("redirect_uri", redirect_uri),
                ],
            )
            .await?;
        serde_json::from_value(v)
            .map_err(|e| OxyError::RuntimeError(format!("oauth.v2.access decode: {e}")))
    }

    pub async fn auth_revoke(&self, bot_token: &str) -> Result<(), OxyError> {
        self.post_form("auth.revoke", Some(bot_token), &[]).await?;
        Ok(())
    }

    pub async fn users_info(&self, bot_token: &str, user_id: &str) -> Result<UserInfo, OxyError> {
        let v = self
            .post_form("users.info", Some(bot_token), &[("user", user_id)])
            .await?;
        serde_json::from_value(v)
            .map_err(|e| OxyError::RuntimeError(format!("users.info decode: {e}")))
    }

    pub async fn chat_post_message(
        &self,
        bot_token: &str,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<Value, OxyError> {
        self.chat_post_message_with_blocks(bot_token, channel, text, thread_ts, None)
            .await
    }

    /// Variant of `chat.postMessage` that also attaches a `blocks` array —
    /// used by the streaming fallback path so the workspace footer card /
    /// error alert still render even when chat.startStream is unavailable.
    pub async fn chat_post_message_with_blocks(
        &self,
        bot_token: &str,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
        blocks: Option<Value>,
    ) -> Result<Value, OxyError> {
        let mut body = serde_json::json!({ "channel": channel, "text": text });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = Value::String(ts.to_string());
        }
        if let Some(blk) = blocks {
            body["blocks"] = blk;
        }
        self.post_json("chat.postMessage", bot_token, &body).await
    }

    pub async fn chat_post_ephemeral(
        &self,
        bot_token: &str,
        channel: &str,
        user: &str,
        blocks: Value,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<Value, OxyError> {
        let mut body = serde_json::json!({
            "channel": channel, "user": user, "text": text, "blocks": blocks,
        });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = serde_json::Value::String(ts.to_string());
        }
        self.post_json("chat.postEphemeral", bot_token, &body).await
    }

    pub async fn views_publish(
        &self,
        bot_token: &str,
        user_id: &str,
        view: Value,
    ) -> Result<Value, OxyError> {
        let body = serde_json::json!({ "user_id": user_id, "view": view });
        self.post_json("views.publish", bot_token, &body).await
    }

    /// Set the native AI-assistant thread status indicator. With
    /// `loading_messages` populated, Slack rotates through the array as
    /// a flashing loading state (cf. Claude's "Generating response…"
    /// pattern). `status` is the static fallback shown alongside
    /// non-rotating clients.
    ///
    /// Pass `status: ""` to clear the indicator.
    /// Slack caps `loading_messages` at 10 entries.
    /// <https://docs.slack.dev/reference/methods/assistant.threads.setStatus>
    pub async fn assistant_threads_set_status(
        &self,
        bot_token: &str,
        channel: &str,
        thread_ts: &str,
        status: &str,
        loading_messages: Option<&[&str]>,
    ) -> Result<(), OxyError> {
        let mut body = serde_json::json!({
            "channel_id": channel,
            "thread_ts": thread_ts,
            "status": status,
        });
        if let Some(msgs) = loading_messages {
            body["loading_messages"] = serde_json::Value::Array(
                msgs.iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect(),
            );
        }
        self.post_json("assistant.threads.setStatus", bot_token, &body)
            .await?;
        Ok(())
    }

    /// Set the human-readable title for an Assistant thread. Slack shows
    /// this in the History tab of the bot's split-view chat — without it,
    /// each thread defaults to the first ~40 chars of the user's question
    /// or a generic placeholder. We call this on the first message of a
    /// new thread so users can scan their conversation history at a glance.
    ///
    /// Docs: <https://docs.slack.dev/reference/methods/assistant.threads.setTitle>
    pub async fn assistant_threads_set_title(
        &self,
        bot_token: &str,
        channel: &str,
        thread_ts: &str,
        title: &str,
    ) -> Result<(), OxyError> {
        let body = serde_json::json!({
            "channel_id": channel,
            "thread_ts": thread_ts,
            "title": title,
        });
        self.post_json("assistant.threads.setTitle", bot_token, &body)
            .await?;
        Ok(())
    }

    /// Set clickable starter prompts in an Assistant thread. Each prompt is a
    /// `(title, message)` pair — title is shown as a button label, message is
    /// what gets sent when the user clicks. Up to 4 prompts are displayed.
    ///
    /// Docs: <https://api.slack.com/methods/assistant.threads.setSuggestedPrompts>
    pub async fn assistant_threads_set_suggested_prompts(
        &self,
        bot_token: &str,
        channel: &str,
        thread_ts: &str,
        prompts: &[(&str, &str)],
    ) -> Result<(), OxyError> {
        let prompts_json: Vec<Value> = prompts
            .iter()
            .map(|(title, message)| serde_json::json!({ "title": title, "message": message }))
            .collect();
        let body = serde_json::json!({
            "channel_id": channel,
            "thread_ts": thread_ts,
            "prompts": prompts_json,
        });
        self.post_json("assistant.threads.setSuggestedPrompts", bot_token, &body)
            .await?;
        Ok(())
    }

    /// Replace the original message that triggered a block-action with a
    /// new block payload, via Slack's `response_url` (pre-authenticated —
    /// no bot token needed). Used by interactivity handlers that want to
    /// transform the prompt into a "loading…" or "done" state without a
    /// separate `chat.update` call (which would need the message ts).
    ///
    /// Failures are logged-and-swallowed; failing to update the original
    /// message is cosmetically bad but not functionally blocking — the
    /// follow-up uploads / replies still happen.
    pub async fn replace_via_response_url(
        &self,
        response_url: &str,
        text: &str,
        blocks: serde_json::Value,
    ) {
        let result = self
            .http
            .post(response_url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&serde_json::json!({
                "replace_original": true,
                "text": text,
                "blocks": blocks,
            }))
            .send()
            .await;
        match result {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                tracing::warn!(
                    status = resp.status().as_u16(),
                    "replace_via_response_url: non-success status"
                );
            }
            Err(e) => {
                tracing::warn!("replace_via_response_url: http error: {e}");
            }
        }
    }

    /// Delete or replace the original message that triggered a block-action
    /// via Slack's `response_url` (pre-authenticated by Slack — no bot token
    /// needed). Posting `{"delete_original": true}` removes the picker from
    /// the channel after the user has made their selection.
    ///
    /// Failures are logged-and-swallowed; failing to delete a picker is
    /// cosmetically bad but not functionally blocking.
    pub async fn delete_via_response_url(&self, response_url: &str) {
        let result = self
            .http
            .post(response_url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&serde_json::json!({ "delete_original": true }))
            .send()
            .await;
        match result {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                tracing::warn!(
                    status = resp.status().as_u16(),
                    "delete_via_response_url: non-success status"
                );
            }
            Err(e) => {
                tracing::warn!("delete_via_response_url: http error: {e}");
            }
        }
    }

    /// Upload a file to Slack via the `files.uploadV2` 3-step protocol
    /// and auto-share it into `channel`/`thread_ts`. Returns the resulting
    /// Slack file id (currently unused by callers — the auto-share is
    /// what surfaces the file in the thread).
    ///
    /// **Why auto-share.** Embedding the file in the same `chat.postMessage`
    /// as the prose via a `slack_file` Block Kit image block is brittle:
    /// the file isn't accessible to channel members until it's shared, so
    /// Slack tends to silently drop the message. The reliable pattern is
    /// to post the prose answer first (no image blocks), then auto-share
    /// each chart as a thread reply. This is what Datadog, Grafana, and
    /// other Slack-native dashboards do.
    ///
    /// The protocol:
    ///
    ///   1. `files.getUploadURLExternal` — returns a pre-authorized upload
    ///      URL plus the file id Slack will use once the bytes land.
    ///   2. multipart POST of the raw bytes to that URL (no bot token —
    ///      the URL itself is the auth).
    ///   3. `files.completeUploadExternal` — finalizes the file and
    ///      auto-shares it into the target channel/thread.
    ///
    /// Docs: <https://docs.slack.dev/reference/methods/files.uploadV2>
    pub async fn files_upload_v2(
        &self,
        bot_token: &str,
        channel: &str,
        thread_ts: Option<&str>,
        filename: &str,
        bytes: Vec<u8>,
        title: Option<&str>,
        content_type: &str,
    ) -> Result<String, OxyError> {
        let length = bytes.len().to_string();
        // `content_type` is undocumented on `files.getUploadURLExternal`
        // but is honored by Slack's edge in practice — sending it makes
        // file classification deterministic instead of relying solely on
        // the filename suffix. Unknown form fields are ignored, so this
        // is safe even if Slack changes its mind later.
        let upload_init = self
            .post_form(
                "files.getUploadURLExternal",
                Some(bot_token),
                &[
                    ("filename", filename),
                    ("length", &length),
                    ("content_type", content_type),
                ],
            )
            .await?;
        let upload_url = upload_init
            .get("upload_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                OxyError::RuntimeError("files.getUploadURLExternal: missing upload_url".to_string())
            })?
            .to_string();
        let file_id = upload_init
            .get("file_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                OxyError::RuntimeError("files.getUploadURLExternal: missing file_id".to_string())
            })?
            .to_string();

        // Step 2: multipart upload. The upload URL is pre-authorized;
        // bearer auth is intentionally NOT sent here.
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(content_type)
            .map_err(|e| OxyError::RuntimeError(format!("multipart mime: {e}")))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = self
            .http
            .post(&upload_url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("files upload POST: {e}")))?;
        if !resp.status().is_success() {
            return Err(OxyError::RuntimeError(format!(
                "files upload POST non-success: {}",
                resp.status()
            )));
        }

        // Step 3: finalize the file and auto-share it into the thread.
        // `files` is a JSON-encoded array even on this form endpoint.
        let mut files_entry = serde_json::json!({ "id": file_id });
        if let Some(t) = title {
            files_entry["title"] = serde_json::Value::String(t.to_string());
        }
        let files_json = serde_json::to_string(&serde_json::json!([files_entry]))
            .map_err(|e| OxyError::RuntimeError(format!("encode files array: {e}")))?;
        let mut form: Vec<(&str, &str)> = vec![("files", &files_json), ("channel_id", channel)];
        if let Some(ts) = thread_ts {
            form.push(("thread_ts", ts));
        }
        self.post_form("files.completeUploadExternal", Some(bot_token), &form)
            .await?;

        Ok(file_id)
    }

    /// Uses the app-level token (`xapp-...`), not the bot token.
    /// Docs: <https://api.slack.com/methods/apps.connections.open>
    pub async fn apps_connections_open(&self, app_level_token: &str) -> Result<String, OxyError> {
        let v = self
            .post_form("apps.connections.open", Some(app_level_token), &[])
            .await?;
        v.get("url")
            .and_then(|u| u.as_str())
            .map(str::to_string)
            .ok_or_else(|| OxyError::RuntimeError("apps.connections.open: missing url".into()))
    }
}

fn check_ok(method: &str, v: &Value) -> Result<(), OxyError> {
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        let err = v.get("error").and_then(Value::as_str).unwrap_or("unknown");
        Err(OxyError::RuntimeError(format!(
            "slack {method} not ok: {err}"
        )))
    }
}

#[derive(Debug, Deserialize)]
pub struct OauthV2Access {
    pub ok: bool,
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    pub bot_user_id: String,
    pub team: OauthTeam,
    pub enterprise: Option<OauthEnterprise>,
    pub authed_user: OauthAuthedUser,
}
#[derive(Debug, Deserialize)]
pub struct OauthTeam {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Deserialize)]
pub struct OauthEnterprise {
    pub id: String,
    pub name: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct OauthAuthedUser {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct UserInfo {
    pub ok: bool,
    pub user: Option<UserInfoUser>,
}
#[derive(Debug, Deserialize)]
pub struct UserInfoUser {
    pub id: String,
    pub profile: Option<UserInfoProfile>,
}
#[derive(Debug, Deserialize)]
pub struct UserInfoProfile {
    pub email: Option<String>,
}

#[cfg(test)]
mod files_upload_v2_tests {
    //! Integration-style tests for the three-step `files.uploadV2`
    //! protocol against a wiremock server. The interesting behaviors
    //! (no bot token on step 2, JSON-encoded `files` array on step 3,
    //! `channel_id` passed to step 3 for auto-share) are exercised by
    //! checking that the bytes round-trip end-to-end and the returned
    //! file id matches what step 1 promised.
    use super::*;
    use wiremock::matchers::{body_string_contains, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn happy_path_returns_file_id_from_get_upload_url() {
        let server = MockServer::start().await;

        // Step 1: hand back an upload URL that points at the mock itself
        // so step 2 hits a path we control.
        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/edge-upload", &server.uri()),
                "file_id": "F0CHART",
            })))
            .mount(&server)
            .await;

        // Step 2: bare 200 — body shape is opaque to the caller.
        Mock::given(method("POST"))
            .and(path("/edge-upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // Step 3: assert the form contains the file id from step 1 plus
        // the channel id that auto-shares into the thread.
        Mock::given(method("POST"))
            .and(path("/files.completeUploadExternal"))
            .and(body_string_contains("F0CHART"))
            .and(body_string_contains("channel_id=C123"))
            .and(body_string_contains("thread_ts="))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(server.uri());
        let file_id = client
            .files_upload_v2(
                "xoxb-test",
                "C123",
                Some("12345.6789"),
                "chart.png",
                vec![0x89, b'P', b'N', b'G'],
                Some("My chart"),
                "image/png",
            )
            .await
            .expect("upload");
        assert_eq!(file_id, "F0CHART");
    }

    #[tokio::test]
    async fn step2_4xx_from_upload_url_surfaces_as_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/edge-upload", &server.uri()),
                "file_id": "F0CHART",
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/edge-upload"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        // Step 3 must not be hit when step 2 fails — `expect(0)` on a
        // wide-open mock (no path filter) catches any stray request.
        Mock::given(method("POST"))
            .and(path("/files.completeUploadExternal"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(server.uri());
        let err = client
            .files_upload_v2(
                "xoxb-test",
                "C123",
                None,
                "chart.png",
                vec![1, 2, 3],
                None,
                "image/png",
            )
            .await
            .expect_err("should surface step-2 4xx");
        let msg = err.to_string();
        assert!(msg.contains("POST"), "got: {msg}");
        assert!(msg.contains("403"), "got: {msg}");
    }

    #[tokio::test]
    async fn step1_missing_upload_url_is_a_typed_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "file_id": "F0CHART",
                // upload_url intentionally absent
            })))
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(server.uri());
        let err = client
            .files_upload_v2(
                "xoxb-test",
                "C123",
                None,
                "chart.png",
                vec![1, 2, 3],
                None,
                "image/png",
            )
            .await
            .expect_err("should reject missing upload_url");
        assert!(err.to_string().contains("missing upload_url"));
    }

    #[tokio::test]
    async fn step2_bypasses_bot_token_auth() {
        // The pre-authorized upload URL must be hit without a bearer
        // token; including one is a real bug in production. Wiremock
        // proves the absence by failing the match on `header_exists`.
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/edge-upload", &server.uri()),
                "file_id": "F0CHART",
            })))
            .mount(&server)
            .await;

        // If the upload step accidentally added bearer auth, this match
        // would succeed and respond 500, failing the call.
        Mock::given(method("POST"))
            .and(path("/edge-upload"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        // The expected path: no auth header.
        Mock::given(method("POST"))
            .and(path("/edge-upload"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/files.completeUploadExternal"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let client = SlackClient::with_base_url(server.uri());
        client
            .files_upload_v2(
                "xoxb-test",
                "C123",
                None,
                "chart.png",
                vec![1, 2, 3],
                None,
                "image/png",
            )
            .await
            .expect("upload should succeed when step 2 is unauthenticated");
    }
}
