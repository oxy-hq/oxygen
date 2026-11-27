//! The three providers against a fake on a loopback port: the request each
//! one really sends for a message with pictures, its answer read back, a
//! request past a bound never sent, and what the inference span is given. A
//! provider's refusals are in `refusal_tests`. No provider is ever called.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::tests::png;
use crate::{
    AnthropicProvider, ImageCompletionError, ImageInput, ImageRequest, LlmClient, LlmError,
    OpenAiCompatProvider, OpenAiProvider,
};

/// The Messages API's stream for one text answer.
const ANTHROPIC_SSE: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":900}}}\n\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"the header \"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"moved\"}}\n\n",
    "event: content_block_stop\n",
    "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

/// The Responses API's stream for one text answer.
const RESPONSES_SSE: &str = concat!(
    "event: response.output_text.delta\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"the header \"}\n\n",
    "event: response.output_text.delta\n",
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"moved\"}\n\n",
    "event: response.completed\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":900,\"output_tokens\":4}}}\n\n",
);

/// Chat Completions' stream for one text answer.
const CHAT_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"the header \"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"moved\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

/// A fake provider answering `route` with `status` and `body`.
async fn fake(route: &str, status: u16, body: &str, content_type: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_raw(body.to_string(), content_type))
        .mount(&server)
        .await;
    server
}

async fn streaming(route: &str, sse: &str) -> MockServer {
    fake(route, 200, sse, "text/event-stream").await
}

fn pictures() -> Vec<ImageInput> {
    vec![
        ImageInput::png(png(1440, 900, 48)).labelled("stores at desk, staging"),
        ImageInput::png(png(1440, 900, 51)).labelled("stores at desk, production"),
    ]
}

fn request(images: &[ImageInput]) -> ImageRequest<'_> {
    ImageRequest {
        system: "Sort the screens.",
        text: "Which changed on purpose?",
        images,
        max_tokens: Some(700),
        private: true,
    }
}

/// A Chat Completions backend at `server`.
fn compat(server: &MockServer, model: &str) -> LlmClient {
    let base = format!("{}/v1", server.uri());
    LlmClient::with_provider(OpenAiCompatProvider::new("sk-fake", model, base))
}

/// The one request `server` received: its headers by name, and its body.
async fn sent(server: &MockServer) -> (wiremock::Request, Value) {
    let mut requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "one request");
    let request = requests.remove(0);
    let body = serde_json::from_slice(&request.body).unwrap();
    (request, body)
}

fn header<'a>(request: &'a wiremock::Request, name: &str) -> &'a str {
    request.headers.get(name).unwrap().to_str().unwrap()
}

#[tokio::test]
async fn anthropic_is_sent_the_pictures_as_base64_image_blocks() {
    let server = streaming("/v1/messages", ANTHROPIC_SSE).await;
    let base = format!("{}/v1", server.uri());
    let client = LlmClient::with_provider(AnthropicProvider::with_base_url(
        "sk-fake",
        "claude-sonnet-5",
        base,
    ));
    let images = pictures();
    let answer = client
        .complete_with_images(&request(&images))
        .await
        .unwrap();
    assert_eq!(answer, "the header moved");

    let (request, body) = sent(&server).await;
    assert_eq!(header(&request, "x-api-key"), "sk-fake");
    assert_eq!(body["model"], "claude-sonnet-5");
    assert_eq!(body["max_tokens"], 700);
    assert_eq!(body["system"][0]["text"], "Sort the screens.");
    assert_eq!(
        body["messages"],
        json!([{ "role": "user", "content": [
            { "type": "text", "text": "stores at desk, staging" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png",
                "data": images[0].base64() } },
            { "type": "text", "text": "stores at desk, production" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png",
                "data": images[1].base64() } },
            { "type": "text", "text": "Which changed on purpose?",
              "cache_control": { "type": "ephemeral" } },
        ] }])
    );
}

#[tokio::test]
async fn openai_is_sent_the_pictures_as_input_image_data_urls() {
    let server = streaming("/v1/responses", RESPONSES_SSE).await;
    let base = format!("{}/v1", server.uri());
    let client =
        LlmClient::with_provider(OpenAiProvider::with_base_url("sk-fake", "gpt-5.5", base));
    let images = pictures();
    let answer = client
        .complete_with_images(&request(&images))
        .await
        .unwrap();
    assert_eq!(answer, "the header moved");

    let (request, body) = sent(&server).await;
    assert_eq!(header(&request, "authorization"), "Bearer sk-fake");
    assert_eq!(body["model"], "gpt-5.5");
    assert_eq!(body["instructions"], "Sort the screens.");
    assert_eq!(
        body["input"],
        json!([{ "type": "message", "role": "user", "content": [
            { "type": "input_text", "text": "stores at desk, staging" },
            { "type": "input_image", "image_url": images[0].data_url() },
            { "type": "input_text", "text": "stores at desk, production" },
            { "type": "input_image", "image_url": images[1].data_url() },
            { "type": "input_text", "text": "Which changed on purpose?" },
        ] }])
    );
    assert!(
        images[0]
            .data_url()
            .starts_with("data:image/png;base64,iVBORw0KGgo")
    );
}

#[tokio::test]
async fn a_chat_completions_backend_is_sent_the_pictures_as_image_url_parts() {
    let server = streaming("/v1/chat/completions", CHAT_SSE).await;
    let base = format!("{}/v1", server.uri());
    let client = LlmClient::with_provider(OpenAiCompatProvider::new("sk-fake", "qwen3-vl", base));
    let images = pictures();
    let answer = client
        .complete_with_images(&request(&images))
        .await
        .unwrap();
    assert_eq!(answer, "the header moved");

    let (request, body) = sent(&server).await;
    assert_eq!(header(&request, "authorization"), "Bearer sk-fake");
    assert_eq!(body["model"], "qwen3-vl");
    assert_eq!(
        body["messages"],
        json!([
            { "role": "system", "content": "Sort the screens." },
            { "role": "user", "content": [
                { "type": "text", "text": "stores at desk, staging" },
                { "type": "image_url", "image_url": { "url": images[0].data_url() } },
                { "type": "text", "text": "stores at desk, production" },
                { "type": "image_url", "image_url": { "url": images[1].data_url() } },
                { "type": "text", "text": "Which changed on purpose?" },
            ] },
        ])
    );
}

#[tokio::test]
async fn a_request_past_a_documented_bound_is_not_sent() {
    let server = streaming("/v1/chat/completions", CHAT_SSE).await;
    let client = compat(&server, "qwen3-vl");
    // 5,600 px square is 30,625 patches of 32 px: past the 30,000 one may be.
    let huge = [ImageInput::png(png(5600, 5600, 64))];
    let refused = client.complete_with_images(&request(&huge)).await;
    assert!(
        matches!(&refused, Err(ImageCompletionError::LimitExceeded(why))
            if why.contains("30625 patches")),
        "{refused:?}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ── What the inference span is given ────────────────────────────────────────

/// Every field recorded on a span while it is installed.
#[derive(Default, Clone)]
struct Capture(Arc<Mutex<HashMap<String, String>>>);

struct Collect<'a>(&'a mut HashMap<String, String>);
impl Visit for Collect<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let said = format!("{value:?}");
        self.0.insert(field.name().to_string(), said);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

type Attributes<'a> = tracing::span::Attributes<'a>;
type Record<'a> = tracing::span::Record<'a>;

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_new_span(&self, attrs: &Attributes<'_>, _: &tracing::span::Id, _: Context<'_, S>) {
        attrs.record(&mut Collect(&mut self.0.lock().unwrap()));
    }
    fn on_record(&self, _: &tracing::span::Id, values: &Record<'_>, _: Context<'_, S>) {
        values.record(&mut Collect(&mut self.0.lock().unwrap()));
    }
}

/// The span fields of one answered call that carried [`pictures`].
async fn recorded(private: bool) -> HashMap<String, String> {
    let server = streaming("/v1/chat/completions", CHAT_SSE).await;
    let client = compat(&server, "qwen3-vl");
    let images = pictures();
    let asked = ImageRequest {
        private,
        ..request(&images)
    };
    let capture = Capture::default();
    let answer = {
        let _guard = tracing::subscriber::set_default(Registry::default().with(capture.clone()));
        client.complete_with_images(&asked).await
    };
    assert!(
        matches!(&answer, Ok(t) if t == "the header moved"),
        "{answer:?}"
    );
    let fields = capture.0.lock().unwrap().clone();
    fields
}

#[tokio::test]
async fn the_span_names_each_picture_and_never_holds_its_bytes() {
    let fields = recorded(false).await;
    let input: Value = serde_json::from_str(&fields["gen_ai.input.messages"]).unwrap();
    assert_eq!(
        input,
        json!([{ "role": "user", "content": [
            { "type": "image", "media_type": "image/png", "bytes": 48,
              "label": "stores at desk, staging" },
            { "type": "image", "media_type": "image/png", "bytes": 51,
              "label": "stores at desk, production" },
            { "type": "text", "text": "Which changed on purpose?" },
        ] }])
    );
    assert_eq!(fields["gen_ai.system_instructions"], "Sort the screens.");
    assert!(fields["gen_ai.output.messages"].contains("the header moved"));
    let all = format!("{fields:?}");
    // Every picture's base64 opens with the PNG signature's.
    assert!(pictures()[0].base64().starts_with("iVBORw0KGgo"));
    assert!(!all.contains("iVBORw0KGgo"), "{all}");
}

#[tokio::test]
async fn a_private_request_records_no_prompt_and_no_answer() {
    let fields = recorded(true).await;
    assert_eq!(fields["gen_ai.input.messages"], "[]");
    assert_eq!(fields["gen_ai.system_instructions"], "");
    let output: Value = serde_json::from_str(&fields["gen_ai.output.messages"]).unwrap();
    assert_eq!(
        output[0]["parts"],
        json!([]),
        "the answer is not on the span"
    );
    assert_eq!(fields["oxy.gen_ai.output.bytes"], "0");
    let all = format!("{fields:?}");
    for kept_out in [
        "Which changed",
        "Sort the screens",
        "the header moved",
        "stores at desk",
        "iVBORw0KGgo",
    ] {
        assert!(!all.contains(kept_out), "{kept_out}: {all}");
    }
    // What the call was, and what it used, is still there.
    assert_eq!(fields["gen_ai.request.model"], "qwen3-vl");
    assert_eq!(fields["gen_ai.request.max_tokens"], "700");
    assert_eq!(fields["gen_ai.response.finish_reasons"], "[\"stop\"]");
}

#[path = "refusal_tests.rs"]
mod refusal;
