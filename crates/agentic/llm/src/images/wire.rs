//! A user message with pictures, in each provider's own wire shape.
//!
//! The same order on every wire: each picture, with its label (when it has
//! one) just before it, then the words. Pictures first is Anthropic's own
//! advice; the label is how a model tells several pictures apart.

use serde_json::{Value, json};

use super::ImageInput;

/// One user message whose content is `parts`.
fn user(parts: Vec<Value>) -> Value {
    json!({ "role": "user", "content": parts })
}

/// The parts of a message: `text_part(label)` then `image_part(image)` for
/// each picture, then `text_part(text)`.
fn parts(
    text: &str,
    images: &[ImageInput],
    text_part: impl Fn(&str) -> Value,
    image_part: impl Fn(&ImageInput) -> Value,
) -> Vec<Value> {
    let mut parts = Vec::with_capacity(images.len() * 2 + 1);
    for image in images {
        if let Some(label) = image.label() {
            parts.push(text_part(label));
        }
        parts.push(image_part(image));
    }
    parts.push(text_part(text));
    parts
}

/// The Anthropic Messages API: an `image` block with a base64 `source`
/// (<https://platform.claude.com/docs/en/build-with-claude/vision>).
pub(crate) fn anthropic_message(text: &str, images: &[ImageInput]) -> Value {
    user(parts(
        text,
        images,
        |text| json!({ "type": "text", "text": text }),
        |image| {
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": image.media_type().mime(),
                    "data": image.base64(),
                },
            })
        },
    ))
}

/// The OpenAI Responses API: an `input_image` part whose `image_url` is a
/// data URL (<https://developers.openai.com/api/docs/guides/images-vision>).
pub(crate) fn openai_responses_message(text: &str, images: &[ImageInput]) -> Value {
    user(parts(
        text,
        images,
        |text| json!({ "type": "input_text", "text": text }),
        |image| json!({ "type": "input_image", "image_url": image.data_url() }),
    ))
}

/// OpenAI Chat Completions, and every backend that implements it: an
/// `image_url` part whose `image_url.url` is "either a URL of the image or
/// the base64 encoded image data"
/// (<https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create>).
pub(crate) fn chat_completions_message(text: &str, images: &[ImageInput]) -> Value {
    user(parts(
        text,
        images,
        |text| json!({ "type": "text", "text": text }),
        |image| json!({ "type": "image_url", "image_url": { "url": image.data_url() } }),
    ))
}

/// What the inference span records in place of the message: every picture
/// named by type, size and label, none of its bytes.
pub(crate) fn recorded_message(text: &str, images: &[ImageInput]) -> Value {
    let mut content: Vec<Value> = images
        .iter()
        .map(|image| {
            json!({
                "type": "image",
                "media_type": image.media_type().mime(),
                "bytes": image.byte_len(),
                "label": image.label(),
            })
        })
        .collect();
    content.push(json!({ "type": "text", "text": text }));
    user(content)
}
