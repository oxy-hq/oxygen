//! The bounds each provider's documentation puts on the pictures of ONE
//! request, and the arithmetic a caller splits a set of pictures with.
//!
//! These are the only bounds this crate applies: there is no budget of its
//! own. Every constant below names the page it was read from (read
//! 2026-11-22); a provider that changes one changes it here.

use super::ImageInput;

/// A megabyte as the providers' pages count one. They do not say which, so
/// this is the smaller reading (10^6): a bound read too low splits a request
/// early, a bound read too high gets it refused.
const MB: usize = 1_000_000;

/// What one picture adds to a request beside its base64: the JSON around it.
const IMAGE_ENVELOPE_BYTES: usize = 256;
/// What a request carries beside its pictures and its words: the model, the
/// envelope, headroom for JSON escaping of the text.
const REQUEST_ENVELOPE_BYTES: usize = 64 * 1024;

/// A stricter side limit that applies to every picture of a request once it
/// carries more than `over` of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManyImages {
    pub over: usize,
    pub max_side_px: u32,
}

/// A bound on a picture's area, counted in square patches of `side_px`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Patches {
    pub side_px: u32,
    pub max: u64,
}

/// The bounds a provider's documentation puts on the pictures of one request.
/// `None` is a bound the provider does not state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLimits {
    /// The most pictures one request carries.
    pub max_images: usize,
    /// The largest one picture may be as it travels, in base64 characters.
    pub max_image_base64_bytes: Option<usize>,
    /// The longest side of one picture, in pixels.
    pub max_side_px: Option<u32>,
    pub many_images: Option<ManyImages>,
    pub max_patches: Option<Patches>,
    /// The largest request body, in bytes.
    pub max_request_bytes: Option<usize>,
}

/// The base64 length of `bytes` bytes.
const fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

/// The Anthropic Messages API on `api.anthropic.com`.
///
/// <https://platform.claude.com/docs/en/build-with-claude/vision#request-limits>:
/// "100 for each request on the API, for models with a 200k-token context
/// window; 600 for each request on the API, for all other models" (the model's
/// window is not known here, so the lower one); "The maximum dimensions for
/// each image are 8000x8000 px"; past 20 images "a stricter per-image
/// dimension limit applies to every image in that request ... either resize
/// each image so that neither dimension exceeds 3,000 px, or keep the request
/// to 20 or fewer image and document blocks"; "The maximum size per image is
/// 10 MB (base64-encoded) on the Claude API".
/// <https://platform.claude.com/docs/en/api/overview#request-size-limits>:
/// "Messages, Token Counting: 128 MB".
const ANTHROPIC: ImageLimits = ImageLimits {
    max_images: 100,
    max_image_base64_bytes: Some(10 * MB),
    max_side_px: Some(8000),
    many_images: Some(ManyImages {
        over: 20,
        max_side_px: 3000,
    }),
    max_patches: None,
    max_request_bytes: Some(128 * MB),
};

/// Anthropic's wire on any other host (`api_url` set: a gateway, or a cloud
/// platform behind one). The same pages give each platform its own bounds and
/// this crate cannot tell which is behind a URL, so it keeps to the strictest
/// they state: "5 MB (base64-encoded) on Amazon Bedrock" a picture, and
/// "Bedrock limits requests to 20 MB".
const ANTHROPIC_ELSEWHERE: ImageLimits = ImageLimits {
    max_image_base64_bytes: Some(5 * MB),
    max_request_bytes: Some(20 * MB),
    ..ANTHROPIC
};

/// OpenAI's own API, for both of its wires (Responses and Chat Completions).
///
/// <https://developers.openai.com/api/docs/guides/images-vision>: "Up to 512 MB
/// total payload per request", "Up to 1,500 images per request", and "up to
/// 30,000 patches per image after applying the resizing rules", an image past
/// that being "rejected, not automatically resized", where a patch is "32px x
/// 32px". The page states no size per image. The resizing rules differ by
/// model, so the patch bound is applied to the picture as it is sent: the
/// strictest reading, exact for a model that does not downscale.
const OPENAI: ImageLimits = ImageLimits {
    max_images: 1500,
    max_image_base64_bytes: None,
    max_side_px: None,
    many_images: None,
    max_patches: Some(Patches {
        side_px: 32,
        max: 30_000,
    }),
    max_request_bytes: Some(512 * MB),
};

/// Azure OpenAI (either wire, on an Azure host).
///
/// <https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/gpt-with-vision#input-limitations>:
/// "The maximum size for input images is restricted to 20 MB" and "you're
/// limited to 10 images per chat call". (Its quotas page gives 50 for two
/// models; this is the lower of the two pages.) Neither states a request size.
const AZURE_OPENAI: ImageLimits = ImageLimits {
    max_images: 10,
    max_image_base64_bytes: Some(base64_len(20 * MB)),
    max_side_px: None,
    many_images: None,
    max_patches: None,
    max_request_bytes: None,
};

/// The bounds of an Anthropic provider that talks to `base_url`.
pub(crate) fn anthropic_limits(base_url: &str) -> ImageLimits {
    let first_party = crate::genai::server_endpoint(base_url)
        .is_some_and(|(host, _)| host.eq_ignore_ascii_case("api.anthropic.com"));
    if first_party {
        ANTHROPIC
    } else {
        ANTHROPIC_ELSEWHERE
    }
}

/// The bounds of a provider that speaks one of OpenAI's two wires, by the
/// provider name its endpoint resolved to (`genai::provider_name_for_url`).
///
/// An OpenAI-compatible backend that is neither OpenAI nor Azure (Ollama, a
/// gateway, a self-hosted server) documents no bounds of its own
/// (<https://docs.ollama.com/api/openai-compatibility> lists vision with a
/// base64 image and states none), so it is held to the API it implements. One
/// that takes fewer pictures refuses the request, which the caller is told as
/// `ImagesNotSupported`.
pub(crate) fn openai_wire_limits(provider_name: &str) -> ImageLimits {
    if provider_name == "azure.ai.openai" {
        AZURE_OPENAI
    } else {
        OPENAI
    }
}

impl ImageLimits {
    /// Why no request of this provider can carry `image`, whatever else it
    /// carries; `None` when one can.
    pub fn refuses(&self, image: &ImageInput) -> Option<String> {
        if let Some(max) = self.max_image_base64_bytes
            && image.base64_len() > max
        {
            return Some(format!(
                "a picture of {} bytes is {} as base64, past the {max} one picture may be",
                image.byte_len(),
                image.base64_len()
            ));
        }
        let (width, height) = image.size()?;
        if let Some(max) = self.max_side_px
            && width.max(height) > max
        {
            return Some(format!(
                "a picture of {width}x{height} px is past {max} px a side"
            ));
        }
        if let Some(patches) = self.max_patches {
            let side = u64::from(patches.side_px.max(1));
            let count = u64::from(width).div_ceil(side) * u64::from(height).div_ceil(side);
            if count > patches.max {
                return Some(format!(
                    "a picture of {width}x{height} px is {count} patches, past {}",
                    patches.max
                ));
            }
        }
        None
    }

    /// Whether ONE request can carry `images` beside `other_bytes` of text.
    /// `Err` names the bound it is past.
    pub fn check<'a>(
        &self,
        images: impl IntoIterator<Item = &'a ImageInput>,
        other_bytes: usize,
    ) -> Result<(), String> {
        let images: Vec<&ImageInput> = images.into_iter().collect();
        if images.len() > self.max_images {
            return Err(format!(
                "{} pictures in one request, past {}",
                images.len(),
                self.max_images
            ));
        }
        if let Some(why) = images.iter().find_map(|image| self.refuses(image)) {
            return Err(why);
        }
        if let Some(many) = self.many_images
            && images.len() > many.over
            && let Some((width, height)) = images
                .iter()
                .filter_map(|image| image.size())
                .find(|(width, height)| (*width).max(*height) > many.max_side_px)
        {
            return Err(format!(
                "{} pictures in one request, and one is {width}x{height} px: past {} pictures \
                 each may be {} px a side at most",
                images.len(),
                many.over,
                many.max_side_px
            ));
        }
        let bytes = request_bytes(&images, other_bytes);
        match self.max_request_bytes {
            Some(max) if bytes > max => Err(format!(
                "a request of about {bytes} bytes, past the {max} one request may be"
            )),
            _ => Ok(()),
        }
    }

    /// Whether a request that already carries `held` can take `more` too,
    /// beside `other_bytes` of text. A caller with more pictures than one
    /// request carries fills a request until this says no, then starts the
    /// next.
    pub fn can_carry(&self, held: &[ImageInput], more: &[ImageInput], other_bytes: usize) -> bool {
        self.check(held.iter().chain(more), other_bytes).is_ok()
    }
}

/// About how large a request with `images` and `other_bytes` of text is.
fn request_bytes(images: &[&ImageInput], other_bytes: usize) -> usize {
    let pictures: usize = images
        .iter()
        .map(|image| image.base64_len() + image.label().map_or(0, str::len) + IMAGE_ENVELOPE_BYTES)
        .sum();
    pictures + other_bytes + REQUEST_ENVELOPE_BYTES
}
