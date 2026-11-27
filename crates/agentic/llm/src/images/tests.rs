//! Image input without a socket: each provider's message, Anthropic's whole
//! request body, the documented bounds, and what a provider with no image
//! input answers. What a provider is really sent, a refusal and what the
//! inference span is given are `http_tests`'.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::limits::{anthropic_limits, openai_wire_limits};
use crate::engine::mock::{MockProvider, Round, Seen};
use crate::{
    AnthropicProvider, Chunk, ImageCompletionError, ImageInput, ImageLimits, ImageMediaType,
    ImageRequest, ImageWire, LlmClient, LlmError, OpenAiCompatProvider, OpenAiProvider,
    ThinkingConfig, Usage,
};

/// A PNG header saying `width` x `height`, padded to `len` bytes.
pub(super) fn png(width: u32, height: u32, len: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.resize(len.max(24), 0);
    bytes
}

/// Two small pictures: a labelled PNG and an unlabelled JPEG.
fn two() -> Vec<ImageInput> {
    vec![
        ImageInput::png(png(1440, 900, 30)).labelled("stores, staging"),
        ImageInput::new(ImageMediaType::Jpeg, vec![1, 2, 3]),
    ]
}

fn request<'a>(images: &'a [ImageInput], private: bool) -> ImageRequest<'a> {
    ImageRequest {
        system: "be brief",
        text: "what changed?",
        images,
        max_tokens: Some(512),
        private,
    }
}

#[test]
fn a_png_says_its_size_and_a_picture_never_prints_its_bytes() {
    let image = ImageInput::png(png(1440, 4000, 64)).labelled("root");
    assert_eq!(image.size(), Some((1440, 4000)));
    assert_eq!((image.byte_len(), image.base64_len()), (64, 88));
    assert_eq!(image.base64().len(), image.base64_len());
    assert_eq!(
        ImageInput::png(b"not a png at all, just bytes".to_vec()).size(),
        None
    );
    assert_eq!(
        ImageInput::new(ImageMediaType::Jpeg, png(1, 1, 30)).size(),
        None
    );
    let printed = format!("{image:?}");
    assert_eq!(
        printed,
        "ImageInput { media_type: \"image/png\", bytes: 64, size: Some((1440, 4000)), \
         label: Some(\"root\") }"
    );
}

#[test]
fn each_provider_gets_the_pictures_in_its_own_wire_shape() {
    let images = two();
    let (png_b64, jpeg_b64) = (images[0].base64(), "AQID");
    let anthropic = ImageWire::Anthropic.message("what changed?", &images);
    assert_eq!(
        anthropic,
        json!({ "role": "user", "content": [
            { "type": "text", "text": "stores, staging" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": png_b64 } },
            { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": jpeg_b64 } },
            { "type": "text", "text": "what changed?" },
        ] })
    );
    let openai = ImageWire::OpenAiResponses.message("what changed?", &images);
    assert_eq!(
        openai,
        json!({ "role": "user", "content": [
            { "type": "input_text", "text": "stores, staging" },
            { "type": "input_image", "image_url": format!("data:image/png;base64,{png_b64}") },
            { "type": "input_image", "image_url": "data:image/jpeg;base64,AQID" },
            { "type": "input_text", "text": "what changed?" },
        ] })
    );
    let compat = ImageWire::ChatCompletions.message("what changed?", &images);
    assert_eq!(
        compat,
        json!({ "role": "user", "content": [
            { "type": "text", "text": "stores, staging" },
            { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{png_b64}") } },
            { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,AQID" } },
            { "type": "text", "text": "what changed?" },
        ] })
    );
}

/// The whole body Anthropic is sent, from the pure builder: the pictures
/// reach `messages` whole, and the cache marker lands on the words, never on
/// a picture.
#[test]
fn anthropics_request_body_carries_the_pictures() {
    let images = two();
    let provider = AnthropicProvider::new("k", "claude-sonnet-5");
    let message = ImageWire::Anthropic.message("what changed?", &images);
    let body = provider.build_request_body(
        "be brief",
        "",
        &[message],
        &[],
        &ThinkingConfig::Disabled,
        None,
        Some(512),
    );
    assert_eq!(body["max_tokens"], 512);
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    let content = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 4);
    assert_eq!(content[1]["source"]["data"], images[0].base64());
    assert_eq!(content[1].get("cache_control"), None);
    assert_eq!(content[2]["source"]["media_type"], "image/jpeg");
    assert_eq!(
        content[3],
        json!({ "type": "text", "text": "what changed?", "cache_control": { "type": "ephemeral" } })
    );
    assert!(body.get("tools").is_none() && body.get("thinking").is_none());
}

/// The bounds of a client built on `provider`.
fn bounds(provider: impl crate::LlmProvider + 'static) -> ImageLimits {
    let client = LlmClient::with_provider(provider);
    client.image_limits().expect("one of the three providers")
}

#[test]
fn each_provider_states_its_documented_bounds() {
    let first_party = bounds(AnthropicProvider::new("k", "claude-sonnet-5"));
    assert_eq!(
        (first_party.max_images, first_party.max_side_px),
        (100, Some(8000))
    );
    assert_eq!(first_party.max_image_base64_bytes, Some(10_000_000));
    assert_eq!(first_party.max_request_bytes, Some(128_000_000));
    let many = first_party.many_images.unwrap();
    assert_eq!((many.over, many.max_side_px), (20, 3000));
    assert_eq!(
        first_party,
        anthropic_limits("https://API.anthropic.com/v1")
    );

    // A gateway: the strictest bounds any documented platform states.
    let gateway = bounds(AnthropicProvider::with_base_url(
        "k",
        "claude-sonnet-5",
        "https://gw.example/v1",
    ));
    assert_eq!(gateway.max_image_base64_bytes, Some(5_000_000));
    assert_eq!(gateway.max_request_bytes, Some(20_000_000));
    assert_eq!((gateway.max_images, gateway.many_images), (100, Some(many)));

    let openai = bounds(OpenAiProvider::new("k", "gpt-5.5"));
    assert_eq!(
        (openai.max_images, openai.max_request_bytes),
        (1500, Some(512_000_000))
    );
    assert_eq!(openai.max_image_base64_bytes, None);
    let patches = openai.max_patches.unwrap();
    assert_eq!((patches.side_px, patches.max), (32, 30_000));
    let ollama = OpenAiCompatProvider::new("k", "qwen3-vl", "http://localhost:11434/v1");
    assert_eq!(bounds(ollama), openai);

    let azure = bounds(OpenAiCompatProvider::for_azure(
        "k",
        "gpt-4.1",
        "https://acme.openai.azure.com",
        "gpt41",
        "2025-03-01-preview",
    ));
    assert_eq!(azure.max_images, 10);
    assert_eq!(azure.max_image_base64_bytes, Some(26_666_668));
    assert_eq!(azure, openai_wire_limits("azure.ai.openai"));
}

/// The stored pictures of a build's checks: a PNG of at most 2,000,000 bytes,
/// 1440 px wide and up to 4,000 px tall.
fn screenshot(height: u32) -> ImageInput {
    ImageInput::png(png(1440, height, 2_000_000))
}

#[test]
fn a_request_is_checked_against_every_bound() {
    let limits = anthropic_limits("https://api.anthropic.com/v1");
    let tall: Vec<ImageInput> = (0..21).map(|_| screenshot(4000)).collect();
    assert_eq!(limits.check(&tall[..20], 0), Ok(()));
    let refused = limits.check(&tall, 0).unwrap_err();
    assert!(
        refused.contains("past 20 pictures each may be 3000 px"),
        "{refused}"
    );
    // Twenty-one that all fit 3,000 px a side ride together.
    let short: Vec<ImageInput> = (0..21).map(|_| screenshot(3000)).collect();
    assert_eq!(limits.check(&short, 0), Ok(()));
    assert!(limits.can_carry(&tall[..19], &tall[..1], 0));
    assert!(!limits.can_carry(&tall[..20], &tall[..1], 0));

    // A gateway's 20 MB request: seven 2 MB pictures (2,666,668 as base64).
    let gateway = anthropic_limits("https://gw.example/v1");
    assert_eq!(gateway.check(&tall[..7], 0), Ok(()));
    let refused = gateway.check(&tall[..8], 0).unwrap_err();
    assert!(
        refused.contains("past the 20000000 one request may be"),
        "{refused}"
    );
    // The words count too.
    assert!(gateway.check(&tall[..7], 2_000_000).is_err());

    let few = ImageLimits {
        max_images: 2,
        ..limits
    };
    let refused = few.check(&tall[..3], 0).unwrap_err();
    assert_eq!(refused, "3 pictures in one request, past 2");
}

#[test]
fn a_picture_no_request_can_carry_is_named() {
    let limits = anthropic_limits("https://api.anthropic.com/v1");
    assert_eq!(limits.refuses(&screenshot(4000)), None);
    let wide = ImageInput::png(png(8001, 100, 64));
    assert_eq!(
        limits.refuses(&wide).unwrap(),
        "a picture of 8001x100 px is past 8000 px a side"
    );
    let heavy = ImageInput::png(png(100, 100, 7_500_001));
    let why = limits.refuses(&heavy).unwrap();
    assert!(
        why.contains("10000004 as base64, past the 10000000"),
        "{why}"
    );
    assert!(limits.check([&heavy], 0).is_err());
    // A size the bytes do not say is not judged: the provider decides.
    assert_eq!(
        limits.refuses(&ImageInput::new(ImageMediaType::Webp, vec![0; 64])),
        None
    );

    let openai = openai_wire_limits("openai");
    // 5,600 px square is 175 x 175 patches of 32 px: 30,625.
    let huge = ImageInput::png(png(5600, 5600, 64));
    assert_eq!(
        openai.refuses(&huge).unwrap(),
        "a picture of 5600x5600 px is 30625 patches, past 30000"
    );
    assert_eq!(openai.refuses(&screenshot(4000)), None, "45 x 125 patches");
}

// ── A provider that is not one of the three ─────────────────────────────────

/// A scripted provider with no image input (the trait's defaults), that
/// answers `rounds` and records what it is sent.
fn no_image_input(rounds: Vec<Round>) -> (LlmClient, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let provider = MockProvider {
        rounds: Mutex::new(rounds.into()),
        seen: seen.clone(),
    };
    (LlmClient::with_provider(provider), seen)
}

/// A provider with no image input answers the typed error and is sent
/// nothing: the words never go out alone in the pictures' place.
#[tokio::test]
async fn a_provider_with_no_image_input_says_so_and_is_sent_nothing() {
    let answer = vec![
        Ok(Chunk::Text("unused".to_string())),
        Ok(Chunk::Done(Usage::default())),
    ];
    let (client, seen) = no_image_input(vec![Round::Chunks(answer)]);
    assert_eq!(client.image_limits(), None);
    let images = two();
    let refused = client.complete_with_images(&request(&images, false)).await;
    assert!(
        matches!(&refused, Err(ImageCompletionError::ImagesNotSupported(why))
            if why == "the custom provider has no image input"),
        "{refused:?}"
    );
    assert!(seen.lock().unwrap().messages.is_empty());

    // The same request without pictures is a plain completion, on any provider.
    let text = client.complete_with_images(&request(&[], false)).await;
    assert!(matches!(&text, Ok(t) if t == "unused"), "{text:?}");
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.messages[0],
        [json!({ "role": "user", "content": "what changed?" })]
    );
    assert_eq!(seen.max_tokens, [Some(512)]);
}

/// Which wire is the provider's type's: each of the three has its own, and a
/// provider this crate does not know has none until one is named for it.
#[test]
fn the_wire_is_decided_by_the_providers_type() {
    assert_eq!(
        ImageWire::of(&AnthropicProvider::new("k", "m")),
        Some(ImageWire::Anthropic)
    );
    assert_eq!(
        ImageWire::of(&OpenAiProvider::new("k", "m")),
        Some(ImageWire::OpenAiResponses)
    );
    let compat = OpenAiCompatProvider::new("k", "m", "http://localhost:11434/v1");
    assert_eq!(ImageWire::of(&compat), Some(ImageWire::ChatCompletions));
    let (unknown, _) = no_image_input(Vec::new());
    assert_eq!(unknown.image_limits(), None);
    let named = unknown.with_image_wire(ImageWire::ChatCompletions);
    assert_eq!(named.image_limits(), Some(openai_wire_limits("custom")));
}

/// A provider that takes pictures and refuses these: the typed error, asked
/// once. A request past a bound it documents is never sent at all.
#[tokio::test]
async fn a_refusal_of_the_pictures_is_typed_and_a_request_past_a_bound_is_not_sent() {
    let refusal = LlmError::Http("image_url is only supported by certain models".into());
    let (client, seen) = no_image_input(vec![Round::Open(refusal)]);
    let client = client.with_image_wire(ImageWire::ChatCompletions);
    let images = two();
    let refused = client.complete_with_images(&request(&images, false)).await;
    assert!(
        matches!(&refused, Err(ImageCompletionError::ImagesNotSupported(why))
            if why == "the provider refused the request that carried the pictures: \
                       image_url is only supported by certain models"),
        "{refused:?}"
    );
    let sent = seen.lock().unwrap().messages.clone();
    assert_eq!(sent.len(), 1, "asked once");
    assert_eq!(sent[0][0]["content"][1]["type"], "image_url");

    // 5,600 px square is 30,625 patches of 32 px: past the 30,000 one may be.
    let huge = [ImageInput::png(png(5600, 5600, 64))];
    let past = client.complete_with_images(&request(&huge, false)).await;
    assert!(
        matches!(&past, Err(ImageCompletionError::LimitExceeded(why))
            if why.contains("30625 patches")),
        "{past:?}"
    );
    assert_eq!(seen.lock().unwrap().messages.len(), 1, "not sent");

    // A bad key is a bad key, pictures or not.
    let (client, _) = no_image_input(vec![Round::Open(LlmError::Auth("bad key".into()))]);
    let client = client.with_image_wire(ImageWire::Anthropic);
    let failed = client.complete_with_images(&request(&images, false)).await;
    assert!(
        matches!(failed, Err(ImageCompletionError::Llm(LlmError::Auth(_)))),
        "{failed:?}"
    );
}

/// A failure of a request with no pictures is the failure it is: only a
/// request that carried pictures can have them refused.
#[tokio::test]
async fn a_failed_request_with_no_pictures_is_never_a_refusal_of_pictures() {
    let (client, _) = no_image_input(vec![Round::Open(LlmError::Http("bad request".into()))]);
    let failed = client.complete_with_images(&request(&[], true)).await;
    assert!(
        matches!(&failed, Err(ImageCompletionError::Llm(LlmError::Http(m))) if m == "bad request"),
        "{failed:?}"
    );
    let (client, _) = no_image_input(vec![Round::Chunks(vec![Ok(Chunk::Done(Usage::default()))])]);
    let empty = client.complete_with_images(&request(&[], true)).await;
    assert!(
        matches!(empty, Err(ImageCompletionError::Llm(LlmError::Parse(_)))),
        "{empty:?}"
    );
}
