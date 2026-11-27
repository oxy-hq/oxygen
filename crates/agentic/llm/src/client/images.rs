//! [`LlmClient::complete_with_images`]: a one-shot completion whose user
//! message carries pictures. See [`crate::images`] for what it promises.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tracing::Instrument as _;

use super::LlmClient;
use crate::genai::{self, ChunkStream, InferenceRequest};
use crate::images::{refusal, wire};
use crate::{
    Chunk, ImageCompletionError, ImageLimits, ImageRequest, ImageWire, LlmError, ThinkingConfig,
};

/// The most of a provider's refusal kept in an [`ImageCompletionError`].
const REFUSAL_MAX_CHARS: usize = 300;

/// A stream's text, kept aside as it passes.
type Aside = Arc<Mutex<String>>;

/// `inner` with each text chunk emptied, and the text it carried kept aside.
///
/// What a private request hands [`genai::observe`]: the span still learns
/// when the first chunk came, the usage and any error, and is shown none of
/// the answer.
fn text_aside(inner: ChunkStream) -> (ChunkStream, Aside) {
    let aside = Aside::default();
    let kept = aside.clone();
    let emptied = async_stream::stream! {
        use tokio_stream::StreamExt as _;
        let mut inner = inner;
        while let Some(item) = inner.next().await {
            match item {
                Ok(Chunk::Text(text)) => {
                    kept.lock().unwrap_or_else(|e| e.into_inner()).push_str(&text);
                    yield Ok(Chunk::Text(String::new()));
                }
                other => yield other,
            }
        }
    };
    (Box::pin(emptied), aside)
}

/// Every text chunk of `stream`, joined.
async fn text_of(stream: ChunkStream) -> Result<String, LlmError> {
    use tokio_stream::StreamExt as _;
    let mut text = String::new();
    let mut stream = std::pin::pin!(stream);
    while let Some(chunk) = stream.next().await {
        if let Chunk::Text(t) = chunk? {
            text.push_str(&t);
        }
    }
    Ok(text)
}

impl LlmClient {
    /// Send pictures on `wire`: for a provider that is not one of this
    /// crate's three and speaks one of their wires.
    pub fn with_image_wire(mut self, wire: ImageWire) -> Self {
        self.image_wire = Some(wire);
        self
    }

    /// The bounds the provider's documentation puts on the pictures of one
    /// request; `None` when this provider takes none.
    pub fn image_limits(&self) -> Option<ImageLimits> {
        let wire = self.image_wire?;
        Some(wire.limits(&*self.provider))
    }

    /// Send one user message that carries `request.images` and then
    /// `request.text`, and return the assistant's text. No tools, no history.
    ///
    /// With no pictures this is a plain completion, on any provider. With
    /// pictures, a provider that has no image input, or whose refusal says
    /// its model has none, answers
    /// [`ImageCompletionError::ImagesNotSupported`]; any other refusal is the
    /// call's failure ([`LlmError::Http`]); a request past the provider's
    /// documented bounds is not sent
    /// ([`ImageCompletionError::LimitExceeded`]).
    pub async fn complete_with_images(
        &self,
        request: &ImageRequest<'_>,
    ) -> Result<String, ImageCompletionError> {
        let messages = vec![self.image_message(request)?];
        let span = self.image_span(request);
        let answered = async {
            // Before the request leaves, so time to first chunk includes the
            // upload, which is most of it with pictures.
            let started = std::time::Instant::now();
            let opened = self.provider.stream(
                request.system,
                "",
                &messages,
                &[],
                &ThinkingConfig::Disabled,
                None,
                request.max_tokens,
            );
            let stream = opened
                .await
                .inspect_err(|e| genai::record_error(&span, e))
                .map_err(|e| refused(e, !request.images.is_empty()))?;
            let (stream, aside) = if request.private {
                text_aside(stream)
            } else {
                (stream, Aside::default())
            };
            let shown = text_of(genai::observe(span.clone(), started, stream)).await?;
            let kept = std::mem::take(&mut *aside.lock().unwrap_or_else(|e| e.into_inner()));
            Ok::<_, ImageCompletionError>(shown + &kept)
        };
        let text = answered.instrument(span.clone()).await?;
        if text.is_empty() {
            let err = LlmError::Parse("no text content in response".into());
            genai::record_error(&span, &err);
            return Err(err.into());
        }
        Ok(text)
    }

    /// The provider-native user message of `request`, or why it cannot be
    /// sent.
    fn image_message(&self, request: &ImageRequest<'_>) -> Result<Value, ImageCompletionError> {
        if request.images.is_empty() {
            return Ok(json!({ "role": "user", "content": request.text }));
        }
        let Some(wire) = self.image_wire else {
            return Err(ImageCompletionError::ImagesNotSupported(format!(
                "the {} provider has no image input",
                self.provider.provider_name()
            )));
        };
        let other_bytes = request.system.len() + request.text.len();
        wire.limits(&*self.provider)
            .check(request.images, other_bytes)
            .map_err(ImageCompletionError::LimitExceeded)?;
        Ok(wire.message(request.text, request.images))
    }

    /// The inference span of `request`: the pictures by type and size, never
    /// their bytes; nothing of the prompt when the request is private.
    fn image_span(&self, request: &ImageRequest<'_>) -> tracing::Span {
        let (system, recorded) = if request.private {
            ("", Vec::new())
        } else {
            let message = wire::recorded_message(request.text, request.images);
            (request.system, vec![message])
        };
        genai::inference_span(
            &*self.provider,
            &self.genai,
            &InferenceRequest {
                system,
                messages: &recorded,
                max_tokens: request.max_tokens,
                tool_count: 0,
                structured_output: false,
                state: None,
                round: None,
            },
        )
    }
}

/// A provider's answer to a request that could not be opened, as this call
/// reports it. [`LlmError::Http`] is every `4xx` that is not an
/// authentication failure or a rate limit. With pictures in the request, one
/// whose words say the model has no image input
/// ([`refusal::says_no_image_input`]) is told as that, and the caller stops
/// sending pictures. Every other one (a prompt too long, a picture the
/// provider will not read, a model that does not exist) stays the failure it
/// is: it says nothing about the next request's pictures.
fn refused(error: LlmError, carries_pictures: bool) -> ImageCompletionError {
    match error {
        LlmError::Http(message) if carries_pictures && refusal::says_no_image_input(&message) => {
            let message: String = message.chars().take(REFUSAL_MAX_CHARS).collect();
            ImageCompletionError::ImagesNotSupported(format!(
                "the provider refused the request that carried the pictures: {message}"
            ))
        }
        other => ImageCompletionError::Llm(other),
    }
}
