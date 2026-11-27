//! Image input for a **one-shot** completion: a user message that carries
//! pictures and text ([`LlmClient::complete_with_images`]).
//!
//! Additive and narrow on purpose: no tool loop, no history, and no change
//! to [`LlmProvider`], to a provider, or to any call that exists. Each
//! provider this crate has is sent the message in its own wire shape
//! ([`ImageWire`]) and held to the bounds its documentation puts on the
//! pictures of one request ([`ImageLimits`]). A caller with more pictures
//! than one request carries makes several ([`ImageLimits::can_carry`]);
//! nothing here downscales, because the workspace has no image decoder.
//!
//! **Which wire is decided once, from the provider's type**, when the client
//! is built ([`ImageWire::of`]): the three providers of this crate each have
//! theirs, and any other provider has none unless whoever builds the client
//! names one ([`LlmClient::with_image_wire`]).
//!
//! **A picture is never dropped silently.** A provider with no wire, and a
//! provider whose refusal says its model has no image input, both answer
//! [`ImageCompletionError::ImagesNotSupported`]; a request past a documented
//! bound answers [`ImageCompletionError::LimitExceeded`] and is not sent. Any
//! other refusal of a request that carried pictures is the failure of that
//! call and says nothing about the next one (`refusal`).
//!
//! **A picture is never recorded.** The inference span of such a call names
//! each picture by type and size and leaves its bytes out. A request marked
//! [`ImageRequest::private`] records no prompt and no answer either: its
//! timing, usage and error class only.
//!
//! [`LlmClient::complete_with_images`]: crate::LlmClient::complete_with_images
//! [`LlmClient::with_image_wire`]: crate::LlmClient::with_image_wire

mod limits;
pub(crate) mod refusal;
pub(crate) mod wire;

#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod tests;

use std::any::TypeId;
use std::fmt;

use base64::Engine as _;
use serde_json::Value;

pub use limits::{ImageLimits, ManyImages, Patches};

use crate::{AnthropicProvider, LlmError, LlmProvider, OpenAiCompatProvider, OpenAiProvider};

/// The wire a provider is sent pictures on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageWire {
    /// The Anthropic Messages API: an `image` block with a base64 source.
    Anthropic,
    /// The OpenAI Responses API: an `input_image` part.
    OpenAiResponses,
    /// OpenAI Chat Completions, and every backend that implements it: an
    /// `image_url` part.
    ChatCompletions,
}

impl ImageWire {
    /// The wire of `provider`, by its type: one of this crate's three, else
    /// none.
    pub(crate) fn of<P: 'static>(_provider: &P) -> Option<Self> {
        let provider = TypeId::of::<P>();
        [
            (TypeId::of::<AnthropicProvider>(), Self::Anthropic),
            (TypeId::of::<OpenAiProvider>(), Self::OpenAiResponses),
            (TypeId::of::<OpenAiCompatProvider>(), Self::ChatCompletions),
        ]
        .into_iter()
        .find_map(|(known, wire)| (known == provider).then_some(wire))
    }

    /// One user message that carries `images` and then `text`, on this wire.
    pub(crate) fn message(self, text: &str, images: &[ImageInput]) -> Value {
        match self {
            Self::Anthropic => wire::anthropic_message(text, images),
            Self::OpenAiResponses => wire::openai_responses_message(text, images),
            Self::ChatCompletions => wire::chat_completions_message(text, images),
        }
    }

    /// The bounds `provider`'s documentation puts on the pictures of one
    /// request: by the host it talks to, for a wire several services speak.
    pub(crate) fn limits(self, provider: &dyn LlmProvider) -> ImageLimits {
        match self {
            Self::Anthropic => limits::anthropic_limits(provider.endpoint().unwrap_or_default()),
            Self::OpenAiResponses | Self::ChatCompletions => {
                limits::openai_wire_limits(provider.provider_name())
            }
        }
    }
}

/// What a picture's bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMediaType {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageMediaType {
    /// The media type every provider's wire names it by.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// The eight bytes every PNG starts with.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A PNG's width and height, from its header: the `IHDR` chunk is always
/// first, so they are bytes 16 to 24. `None` for anything else.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let header = bytes.get(..24)?;
    if header[..8] != PNG_SIGNATURE || &header[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    Some((be(16), be(20)))
}

/// One picture of a user message.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageInput {
    media_type: ImageMediaType,
    bytes: Vec<u8>,
    size: Option<(u32, u32)>,
    label: Option<String>,
}

impl ImageInput {
    pub fn new(media_type: ImageMediaType, bytes: Vec<u8>) -> Self {
        let size = match media_type {
            ImageMediaType::Png => png_size(&bytes),
            _ => None,
        };
        Self {
            media_type,
            bytes,
            size,
            label: None,
        }
    }

    /// A PNG; its width and height are read from its header.
    pub fn png(bytes: Vec<u8>) -> Self {
        Self::new(ImageMediaType::Png, bytes)
    }

    /// Words sent just before the picture, so the model can tell several
    /// apart (`Screen "stores" at desk, staging`).
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn media_type(&self) -> ImageMediaType {
        self.media_type
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// The picture's size in bytes, as it is stored.
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Its size as it travels: base64, four characters for every three bytes.
    pub fn base64_len(&self) -> usize {
        self.bytes.len().div_ceil(3) * 4
    }

    /// Width and height in pixels, when the bytes say (a PNG's header does).
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    pub(crate) fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }

    pub(crate) fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type.mime(), self.base64())
    }
}

/// Never the bytes: a picture can show an org's data, and `Debug` ends up in
/// logs.
impl fmt::Debug for ImageInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageInput")
            .field("media_type", &self.media_type.mime())
            .field("bytes", &self.bytes.len())
            .field("size", &self.size)
            .field("label", &self.label)
            .finish()
    }
}

/// One one-shot completion whose user message may carry pictures.
#[derive(Clone, Copy, Debug)]
pub struct ImageRequest<'a> {
    pub system: &'a str,
    /// The user's words, sent after the pictures.
    pub text: &'a str,
    /// The pictures, in order. None is a plain text completion, which every
    /// provider takes.
    pub images: &'a [ImageInput],
    /// A cap on the answer, where the provider takes one (Anthropic does; the
    /// OpenAI providers ignore it, as they do on every other call).
    pub max_tokens: Option<u32>,
    /// Keep the prompt and the answer off the inference span. For a prompt
    /// that quotes what an org's own pages show: the span then carries the
    /// call's timing, usage and error class, and no content.
    pub private: bool,
}

/// Why [`LlmClient::complete_with_images`] did not answer.
///
/// [`LlmClient::complete_with_images`]: crate::LlmClient::complete_with_images
#[derive(Debug)]
pub enum ImageCompletionError {
    /// The provider takes no pictures: it has no image input, or its refusal
    /// of the request that carried them says its model has none. The same
    /// request without pictures may still be answered, which is what a caller
    /// falls back to, and no later request need carry any. A refusal that
    /// does not say so is [`Self::Llm`] ([`LlmError::Http`]).
    ImagesNotSupported(String),
    /// The request is past a bound the provider documents
    /// ([`ImageLimits`]); it was not sent. A caller splits it.
    LimitExceeded(String),
    /// Any other failure of the call.
    Llm(LlmError),
}

impl fmt::Display for ImageCompletionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImagesNotSupported(why) => write!(f, "images not supported: {why}"),
            Self::LimitExceeded(why) => write!(f, "image limit exceeded: {why}"),
            Self::Llm(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ImageCompletionError {}

impl From<LlmError> for ImageCompletionError {
    fn from(e: LlmError) -> Self {
        Self::Llm(e)
    }
}
