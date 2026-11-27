//! Reading a provider's refusal of a request that carried pictures: does it
//! say the model has **no image input**, or did it refuse this one request?
//!
//! The difference decides what a caller may do next. A model with no image
//! input will refuse every picture, so a caller stops sending them
//! ([`ImageCompletionError::ImagesNotSupported`]). Any other `4xx` (a prompt
//! too long, a picture too large or unreadable, too many pictures, a model
//! that does not exist, an account out of credit) says nothing about the next
//! request: it stays the failure of this call ([`LlmError::Http`]), and a
//! caller that drops its pictures for good over it has stopped showing a
//! model that can see.
//!
//! A `4xx` carries no field that says which it is, on any of the three wires,
//! so the provider's own words are read ([`says_no_image_input`]). The rule
//! is narrow on purpose: a refusal it does not recognise is told as a failure
//! of the call, which costs a caller one retry without the pictures; a
//! refusal it wrongly recognised would cost every later picture.
//!
//! [`ImageCompletionError::ImagesNotSupported`]: super::ImageCompletionError::ImagesNotSupported
//! [`LlmError::Http`]: crate::LlmError::Http

/// What a refusal names when it is about image input.
const IMAGE_WORDS: [&str; 4] = ["image", "vision", "multimodal", "multi-modal"];

/// How a refusal says the model does not take what it names.
const DENIALS: [&str; 11] = [
    "not support",
    "unsupported",
    "only supported",
    "no support",
    "not accept",
    "not take",
    "not available",
    "not enabled",
    "no endpoints found",
    "unknown variant",
    "does not match any of the expected tags",
];

/// A backend whose messages are text only says so of the content it was sent,
/// without naming what it will not take.
const TEXT_ONLY: [&str; 3] = [
    "content must be a string",
    "content` must be a string",
    "content: invalid type: sequence, expected a string",
];

/// Whether `refusal`, a provider's answer to a request that carried pictures,
/// says its model has no image input: it names image input and says the model
/// does not take it, or it says a message's content can only be text.
pub(crate) fn says_no_image_input(refusal: &str) -> bool {
    let said = refusal.to_ascii_lowercase();
    let any = |words: &[&str]| words.iter().any(|word| said.contains(word));
    (any(&IMAGE_WORDS) && any(&DENIALS)) || any(&TEXT_ONLY)
}

#[cfg(test)]
mod tests {
    use super::says_no_image_input;

    #[test]
    fn a_refusal_that_says_the_model_takes_no_images_is_read_as_one() {
        for said in [
            "Invalid content type. image_url is only supported by certain models.",
            "This model does not support image inputs.",
            "Image input is not supported for this model",
            "claude-x does not support image input",
            "The model does not support vision",
            "this model does not take images",
            "Multimodal inputs are unsupported by this deployment",
            "No endpoints found that support image input",
            "Failed to deserialize the JSON body into the target type: messages[0]: \
             unknown variant `image_url`, expected `text`",
            "messages.0.content.0: Input tag 'image' found using 'type' does not match any of \
             the expected tags: 'text'",
            "'messages.0' : for 'role:user' the following must be satisfied[('messages.0.content' \
             : value must be a string)]: `messages[0].content` must be a string",
        ] {
            assert!(says_no_image_input(said), "{said}");
        }
    }

    #[test]
    fn a_refusal_of_this_request_or_these_pictures_is_not() {
        for said in [
            "prompt is too long: 214015 tokens > 200000 maximum",
            "image exceeds 5 MB maximum: 6291456 bytes > 5242880 bytes",
            "Could not process image",
            "At most 20 images may be provided in one request.",
            "image dimensions exceed max allowed size: 8000 pixels",
            "The model `claude-x` does not exist or you do not have access to it.",
            "Your credit balance is too low to access the Anthropic API.",
            "max_tokens: 150000 > 64000, which is the maximum allowed",
            "HTTP 404 Not Found: ",
            "",
        ] {
            assert!(!says_no_image_input(said), "{said}");
        }
    }
}
