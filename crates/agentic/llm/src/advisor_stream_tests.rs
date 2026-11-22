//! `AnthropicProvider` with an advisor, against a local server that answers
//! one request with a canned SSE stream: the request it sends and the chunks
//! it parses.

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::{
    AdvisorConfig, AnthropicProvider, Chunk, ContentBlock, LlmProvider, StopReason, ThinkingConfig,
};

/// Serves `sse` once and returns the request it received (head, body).
async fn serve_once(sse: String) -> (String, tokio::task::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let (head, body) = loop {
            let n = sock.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if body.len() >= len {
                    break (head.to_string(), body.to_string());
                }
            }
        };
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{sse}",
            sse.len()
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
        (head, serde_json::from_str(&body).unwrap())
    });
    (url, handle)
}

fn event(v: Value) -> String {
    format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())
}

/// The docs' shape for a call that pauses: text, the advisor's
/// `server_tool_use`, then `pause_turn` with the advisor in `iterations`.
fn paused_stream() -> String {
    let server_tool_use =
        json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "advisor", "input": {}});
    let result = json!({
        "type": "advisor_tool_result", "tool_use_id": "srvtoolu_1",
        "content": {"type": "advisor_redacted_result", "encrypted_content": "enc"}
    });
    [
        json!({"type": "message_start", "message": {"usage": {"input_tokens": 412, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Checking."}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": server_tool_use}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "ping"}),
        json!({"type": "content_block_start", "index": 2, "content_block": result}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "message_delta", "delta": {"stop_reason": "pause_turn"}, "usage": {
            "input_tokens": 1760, "output_tokens": 531,
            "iterations": [
                {"type": "message", "input_tokens": 412, "output_tokens": 89},
                {"type": "advisor_message", "model": "claude-opus-5-5", "input_tokens": 823,
                 "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 1612}
            ]
        }}),
        json!({"type": "message_stop"}),
    ]
    .into_iter()
    .map(event)
    .collect()
}

#[tokio::test]
async fn advisor_blocks_are_kept_verbatim_and_its_usage_is_separate() {
    use tokio_stream::StreamExt as _;
    let (url, server) = serve_once(paused_stream()).await;
    let provider = AnthropicProvider::with_base_url("k", "claude-sonnet-5-5", url).with_advisor(
        AdvisorConfig {
            model: "claude-opus-5-5".into(),
            max_uses: Some(3),
            max_tokens: Some(2048),
        },
    );
    let msgs = [json!({"role": "user", "content": "q"})];
    let stream = provider
        .stream("sys", "", &msgs, &[], &ThinkingConfig::Adaptive, None, None)
        .await
        .unwrap();
    let chunks: Vec<Chunk> = stream.map(Result::unwrap).collect().await;

    let blocks: Vec<&Value> = chunks
        .iter()
        .filter_map(|c| match c {
            Chunk::RawBlock(ContentBlock::ServerTool { provider_data }) => Some(provider_data),
            _ => None,
        })
        .collect();
    assert_eq!(blocks.len(), 2, "{chunks:?}");
    assert_eq!(blocks[0]["type"], "server_tool_use");
    assert_eq!(blocks[0]["input"], json!({}));
    assert_eq!(blocks[1]["type"], "advisor_tool_result");
    assert_eq!(blocks[1]["content"]["encrypted_content"], "enc");
    let Some(Chunk::Done(usage)) = chunks.last() else {
        panic!("no Done chunk: {chunks:?}")
    };
    assert_eq!(usage.stop_reason, StopReason::PauseTurn);
    assert_eq!(
        usage.input_tokens, 1760,
        "the final count, not message_start's"
    );
    assert_eq!(usage.advisor.calls, 1);
    assert_eq!(usage.advisor.model, "claude-opus-5-5");
    assert_eq!(
        (usage.advisor.input_tokens, usage.advisor.output_tokens),
        (823, 1612)
    );

    let (head, body) = server.await.unwrap();
    let beta = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("anthropic-beta:")
                .map(|v| v.trim().to_string())
        })
        .unwrap();
    assert!(beta.contains("advisor-tool-2026-03-01"), "{beta}");
    assert!(beta.contains("interleaved-thinking"), "{beta}");
    assert_eq!(body["tools"][0]["type"], "advisor_20260301");
    assert_eq!(body["tools"][0]["model"], "claude-opus-5-5");
    assert_eq!(body["tools"][0]["max_uses"], 3);
}

/// The advisor goes first, so the cache breakpoint stays on the last client
/// tool; with no advisor nothing changes.
#[test]
fn the_advisor_tool_leads_and_the_breakpoint_stays_on_a_client_tool() {
    use agentic_core::tools::ToolDef;
    let tool = ToolDef {
        name: "query".into(),
        description: "q".into(),
        parameters: json!({"type": "object", "properties": {}}),
        strict: false,
    };
    let advised = AnthropicProvider::new("k", "claude-sonnet-5-5").with_advisor(AdvisorConfig {
        model: "claude-opus-5-5".into(),
        max_uses: None,
        max_tokens: None,
    });
    let body = advised.build_request_body(
        "s",
        "",
        &[],
        std::slice::from_ref(&tool),
        &ThinkingConfig::Adaptive,
        None,
        None,
    );
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["type"], "advisor_20260301");
    assert!(tools[0].get("cache_control").is_none());
    assert_eq!(tools[1]["name"], "query");
    assert_eq!(tools[1]["cache_control"]["type"], "ephemeral");

    let plain = AnthropicProvider::new("k", "claude-sonnet-5-5");
    let body =
        plain.build_request_body("s", "", &[], &[tool], &ThinkingConfig::Adaptive, None, None);
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
}
