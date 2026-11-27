//! What a provider's `4xx` to a request that carried pictures is told as:
//! `ImagesNotSupported` only when its words say the model has no image input,
//! a failure of the call otherwise. Against the fakes of `http_tests`.

use super::*;

/// A model with no image input answers `400`; each provider's refusal reaches
/// the caller as the typed error, and the same refusal of a request with no
/// pictures does not.
#[tokio::test]
async fn a_provider_that_refuses_the_pictures_is_told_as_images_not_supported() {
    let refusal = json!({ "error": { "type": "invalid_request_error",
        "message": "Invalid content type. image_url is only supported by certain models." } })
    .to_string();
    let images = pictures();

    let server = fake("/v1/chat/completions", 400, &refusal, "application/json").await;
    let base = format!("{}/v1", server.uri());
    let compat = LlmClient::with_provider(OpenAiCompatProvider::new("k", "text-only", base));
    let refused = compat.complete_with_images(&request(&images)).await;
    assert!(
        matches!(&refused, Err(ImageCompletionError::ImagesNotSupported(why))
            if why.ends_with("image_url is only supported by certain models.")),
        "{refused:?}"
    );
    let plain = compat.complete_with_images(&request(&[])).await;
    assert!(
        matches!(plain, Err(ImageCompletionError::Llm(LlmError::Http(_)))),
        "{plain:?}"
    );

    let server = fake("/v1/responses", 400, &refusal, "application/json").await;
    let base = format!("{}/v1", server.uri());
    let openai = LlmClient::with_provider(OpenAiProvider::with_base_url("k", "o3-mini", base));
    let refused = openai.complete_with_images(&request(&images)).await;
    assert!(
        matches!(refused, Err(ImageCompletionError::ImagesNotSupported(_))),
        "{refused:?}"
    );

    let server = fake("/v1/messages", 400, &refusal, "application/json").await;
    let base = format!("{}/v1", server.uri());
    let claude = LlmClient::with_provider(AnthropicProvider::with_base_url("k", "claude-x", base));
    let refused = claude.complete_with_images(&request(&images)).await;
    assert!(
        matches!(refused, Err(ImageCompletionError::ImagesNotSupported(_))),
        "{refused:?}"
    );

    // A key the provider does not know is a failure of the call, not of the
    // pictures.
    let server = fake("/v1/messages", 401, "{}", "application/json").await;
    let base = format!("{}/v1", server.uri());
    let claude = LlmClient::with_provider(AnthropicProvider::with_base_url("k", "claude-x", base));
    let failed = claude.complete_with_images(&request(&images)).await;
    assert!(
        matches!(failed, Err(ImageCompletionError::Llm(LlmError::Auth(_)))),
        "{failed:?}"
    );
}

/// A `4xx` that does not say the model takes no images is a failure of the
/// call, pictures or not: a prompt too long, a picture too large or
/// unreadable, a model that does not exist, an account out of credit. The
/// caller is told what failed, and nothing says the provider takes no
/// pictures.
#[tokio::test]
async fn a_refusal_that_is_not_about_image_input_stays_a_failure_of_the_call() {
    let images = pictures();
    for said in [
        "prompt is too long: 214015 tokens > 200000 maximum",
        "image exceeds 5 MB maximum: 6291456 bytes > 5242880 bytes",
        "Could not process image",
        "At most 20 images may be provided in one request.",
        "The model `claude-x` does not exist or you do not have access to it.",
        "Your credit balance is too low to access the Anthropic API.",
    ] {
        let body = json!({ "error": { "type": "invalid_request_error", "message": said } });
        let server = fake("/v1/messages", 400, &body.to_string(), "application/json").await;
        let base = format!("{}/v1", server.uri());
        let claude =
            LlmClient::with_provider(AnthropicProvider::with_base_url("k", "claude-x", base));
        let failed = claude.complete_with_images(&request(&images)).await;
        assert!(
            matches!(&failed, Err(ImageCompletionError::Llm(LlmError::Http(told))) if told == said),
            "{said:?}: {failed:?}"
        );
    }
}
