//! `POST /world-model/llm/messages`: one LLM turn for a standalone app.

use axum::Json;
use axum::extract::Extension;
use axum::http::StatusCode;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use agentic_core::tools::ToolDef;
use agentic_llm::{AnthropicProvider, Chunk, LlmProvider, ThinkingConfig};
use agentic_pipeline::platform::PlatformContext;

// ── LLM proxy ────────────────────────────────────────────────────────────────
//
// A thin, single-shot LLM passthrough for standalone apps (the world-model
// voice assistant) that must NOT ship a raw provider key in the browser. The
// client sends an Anthropic-style Messages payload (`system` + `messages` +
// optional `tools`); we resolve the workspace's Anthropic key server-side
// (same secret the analytics pipeline uses), call the provider for ONE turn,
// and return the raw content blocks (`text` + `tool_use`) verbatim. There is no
// tool-execution loop — the client runs the returned actions itself.
//
// The model is pinned server-side and `max_tokens` is clamped: the `X-API-Key`
// is client-visible (it ships in the standalone app), so treat this as an
// untrusted, client-exposed token. There is no per-token rate limiting here —
// the model pin + token clamp only stop callers requesting arbitrary/expensive
// models or oversized completions; they do not throttle volume.

/// Pinned, tool-capable Anthropic model configured for this workspace
/// (`config.yml` → `claude-sonnet-4-6`, `key_var: ANTHROPIC_API_KEY`). The
/// client's `model` field is ignored.
const PROXY_MODEL: &str = "claude-sonnet-4-6";
/// Hard ceiling on `max_tokens` regardless of what the client requests.
const PROXY_MAX_TOKENS_CAP: u32 = 2048;
/// Upper bound on the number of distinct tool name/description strings the
/// interner will ever leak (see [`intern`]). Far above the fixed voice
/// vocabulary (~14 tools); trips only on abuse.
const MAX_INTERNED_STRINGS: usize = 1024;

fn default_max_tokens() -> u32 {
    1024
}

/// Inbound Anthropic-style Messages request. Unknown fields (e.g. the client's
/// `model`) are ignored — the model is pinned server-side.
#[derive(Debug, Deserialize)]
pub struct LlmProxyRequest {
    #[serde(default)]
    system: String,
    /// Provider-native message turns (`{role, content}`). Forwarded verbatim.
    #[serde(default)]
    messages: Vec<Value>,
    /// Anthropic tool definitions (`{name, description, input_schema}`). Empty
    /// for a plain text completion (e.g. the spoken-summary call).
    #[serde(default)]
    tools: Vec<LlmProxyTool>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct LlmProxyTool {
    name: String,
    #[serde(default)]
    description: String,
    input_schema: Value,
}

/// `POST /world-model/llm/messages` — proxy one LLM turn to the provider.
///
/// Auth + workspace/project context come from the external router's
/// middleware; `Extension<Arc<dyn PlatformContext>>` is injected by
/// `workspace_middleware` (cloud) / `local_context_middleware` (local), so the
/// Anthropic key is resolved per-workspace exactly like the agentic pipeline.
#[tracing::instrument(skip_all)]
pub async fn proxy_llm_messages(
    // `Option` (not a bare `Extension`): the platform context is injected by the
    // workspace/local-context middleware only once the workspace config
    // resolves. A workspace with no resolvable config never gets it, so a bare
    // extractor would reject with an opaque 500 — surface a 503 instead.
    platform: Option<Extension<Arc<dyn PlatformContext>>>,
    Json(req): Json<LlmProxyRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let Extension(platform) = platform.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "workspace context is unavailable".to_string(),
    ))?;

    // Anthropic requires at least one message; reject early with a clear 400
    // rather than forwarding an empty array and surfacing it as a 502.
    if req.messages.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "messages must be non-empty".to_string(),
        ));
    }

    // Resolve the workspace's Anthropic key (honors the cloud secret store, not
    // just a process env var) — same path the analytics runs take.
    let api_key = platform
        .resolve_secret("ANTHROPIC_API_KEY")
        .await
        .filter(|k| !k.is_empty())
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "ANTHROPIC_API_KEY is not configured for this workspace".to_string(),
        ))?;

    let max_tokens = req.max_tokens.clamp(1, PROXY_MAX_TOKENS_CAP);
    let provider = AnthropicProvider::new(api_key, PROXY_MODEL);

    // `ToolDef` requires `'static` name/description; intern each distinct string
    // once (bounded — see `intern`). A full interner (abuse via unique strings)
    // rejects the request rather than leaking unbounded memory.
    let mut tools: Vec<ToolDef> = Vec::with_capacity(req.tools.len());
    for t in &req.tools {
        let (Some(name), Some(description)) = (intern(&t.name), intern(&t.description)) else {
            return Err((
                StatusCode::BAD_REQUEST,
                "too many distinct tool definitions".to_string(),
            ));
        };
        tools.push(ToolDef {
            name,
            description,
            parameters: t.input_schema.clone(),
            // Don't enforce strict structured-output validation on arbitrary
            // client schemas — Anthropic doesn't need it for tool-use here.
            strict: false,
        });
    }

    let mut stream = provider
        .stream(
            &req.system,
            "",
            &req.messages,
            &tools,
            &ThinkingConfig::Disabled,
            None,
            Some(max_tokens),
        )
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("LLM provider error: {e}")))?;

    // Fold the chunk stream into Anthropic Messages response shape: a single
    // text block (if any) followed by the tool_use blocks. The client reads
    // `content[].type` ("text" → concat, "tool_use" → action), so order within
    // is irrelevant beyond grouping.
    let mut text = String::new();
    let mut tool_blocks: Vec<Value> = Vec::new();
    let mut usage: Option<Value> = None;
    let mut stop_reason = "end_turn";

    while let Some(chunk) = stream.next().await {
        match chunk.map_err(|e| (StatusCode::BAD_GATEWAY, format!("LLM stream error: {e}")))? {
            Chunk::Text(t) => text.push_str(&t),
            Chunk::ToolCall(tc) => {
                stop_reason = "tool_use";
                tool_blocks.push(json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.input,
                }));
            }
            Chunk::Done(u) => {
                usage = Some(json!({
                    "input_tokens": u.input_tokens,
                    "output_tokens": u.output_tokens,
                }));
            }
            Chunk::ThinkingSummary(_) | Chunk::RawBlock(_) => {}
        }
    }

    let mut content: Vec<Value> = Vec::new();
    if !text.is_empty() {
        content.push(json!({ "type": "text", "text": text }));
    }
    content.extend(tool_blocks);

    Ok(Json(json!({
        "content": content,
        "stop_reason": stop_reason,
        "usage": usage,
    })))
}

/// Intern a request-supplied string into a `&'static str` for [`ToolDef`],
/// returning `None` once the interner is full.
///
/// The voice client sends a fixed, small set of tool names/descriptions, so the
/// cache leaks each distinct string at most once. The [`MAX_INTERNED_STRINGS`]
/// cap bounds total leakage: a misbehaving client that spams unique strings
/// stops growing memory once the cap is hit (the caller then 400s) instead of
/// leaking unbounded heap.
fn intern(s: &str) -> Option<&'static str> {
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("intern cache mutex poisoned");
    if let Some(&existing) = guard.get(s) {
        return Some(existing);
    }
    if guard.len() >= MAX_INTERNED_STRINGS {
        return None;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    guard.insert(s.to_owned(), leaked);
    Some(leaked)
}
