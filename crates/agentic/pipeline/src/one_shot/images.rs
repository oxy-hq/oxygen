//! Pictures on a prepared one-shot: [`OneShotCompleter::complete_with_images`],
//! beside [`OneShotCompleter::complete`].
//!
//! What it promises is the model layer's (`agentic_llm::images`): one
//! message, no tools, no history; a provider that takes no pictures, or whose
//! refusal says its model has no image input, is told as
//! [`OneShotImageError::ImagesNotSupported`], never by sending the words
//! alone; any other refusal of the request is a failed call of class `http`
//! ([`OneShotImageError::Failed`]), which says nothing about the next
//! request's pictures; the only bounds are the provider's own
//! documented ones ([`OneShotCompleter::image_limits`]); a picture is never
//! recorded on the inference span, and a request marked `private` records no
//! prompt and no answer either.

pub use agentic_llm::{ImageInput, ImageLimits, ImageMediaType, ImageRequest};

use agentic_llm::ImageCompletionError;

use super::OneShotCompleter;

/// Why [`OneShotCompleter::complete_with_images`] did not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OneShotImageError {
    /// The provider, or its model, did not take the pictures. The same
    /// request without them may still be answered.
    ImagesNotSupported(String),
    /// The request is past a bound the provider documents; it was not sent.
    LimitExceeded(String),
    /// The call failed otherwise. `class` is the low-cardinality class the
    /// inference span records (`auth`, `rate_limit`, `transient`, `http`,
    /// `parse`, ...); `message` is the provider's own words, which a caller
    /// that stores what it shows people keeps out of what it stores.
    Failed {
        class: &'static str,
        message: String,
    },
}

impl std::fmt::Display for OneShotImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ImagesNotSupported(why) => write!(f, "images not supported: {why}"),
            Self::LimitExceeded(why) => write!(f, "image limit exceeded: {why}"),
            Self::Failed { class, message } => {
                write!(f, "the model call failed ({class}): {message}")
            }
        }
    }
}

impl std::error::Error for OneShotImageError {}

impl From<ImageCompletionError> for OneShotImageError {
    fn from(error: ImageCompletionError) -> Self {
        match error {
            ImageCompletionError::ImagesNotSupported(why) => Self::ImagesNotSupported(why),
            ImageCompletionError::LimitExceeded(why) => Self::LimitExceeded(why),
            ImageCompletionError::Llm(e) => Self::Failed {
                class: agentic_llm::genai::error_type(&e),
                message: e.to_string(),
            },
        }
    }
}

impl OneShotCompleter {
    /// The bounds the model's provider documents for the pictures of one
    /// request; `None` when it takes none.
    pub fn image_limits(&self) -> Option<ImageLimits> {
        self.client.image_limits()
    }

    /// Run one completion whose user message carries `request.images` and
    /// then `request.text`. With no pictures it is a plain completion.
    pub async fn complete_with_images(
        &self,
        request: &ImageRequest<'_>,
    ) -> Result<String, OneShotImageError> {
        self.client
            .complete_with_images(request)
            .await
            .map_err(OneShotImageError::from)
    }
}

#[cfg(test)]
mod tests {
    use agentic_analytics::config::{LlmVendor, ResolvedModelInfo};
    use agentic_llm::LlmError;
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::prepare_one_shot;

    /// A workspace whose default model is an OpenAI-compatible backend at
    /// `base_url`.
    struct Workspace {
        base_url: String,
    }

    #[async_trait::async_trait]
    impl crate::platform::ProjectContext for Workspace {
        async fn resolve_connector(&self, _: &str) -> Option<agentic_connector::ConnectorConfig> {
            None
        }
        async fn resolve_model(&self, _: Option<&str>, _: bool) -> Option<ResolvedModelInfo> {
            Some(ResolvedModelInfo {
                model: "qwen3-vl".to_string(),
                vendor: LlmVendor::OpenAiCompat,
                api_key: Some("sk-fake".to_string()),
                base_url: Some(self.base_url.clone()),
                azure_deployment_id: None,
                azure_api_version: None,
                headers: None,
            })
        }
        async fn resolve_secret(&self, _: &str) -> Option<String> {
            None
        }
    }

    const CHAT_SSE: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"two screens moved\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    async fn backend(status: u16, body: &str, content_type: &str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), content_type),
            )
            .mount(&server)
            .await;
        server
    }

    async fn completer(server: &MockServer) -> OneShotCompleter {
        let workspace = Workspace {
            base_url: format!("{}/v1", server.uri()),
        };
        prepare_one_shot(&workspace, None, "promote-checks")
            .await
            .expect("the workspace's default model")
    }

    fn picture() -> ImageInput {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.resize(40, 7);
        ImageInput::png(bytes).labelled("stores at desk, staging")
    }

    fn request(images: &[ImageInput]) -> ImageRequest<'_> {
        ImageRequest {
            system: "Sort the screens.",
            text: "Which changed on purpose?",
            images,
            max_tokens: None,
            private: true,
        }
    }

    /// The workspace's own model, through the facade, on a loopback port: the
    /// picture is in the request it gets, and its answer comes back.
    #[tokio::test]
    async fn a_prepared_one_shot_sends_the_pictures_to_the_workspaces_model() {
        let server = backend(200, CHAT_SSE, "text/event-stream").await;
        let completer = completer(&server).await;
        assert_eq!(completer.model(), "qwen3-vl");
        assert_eq!(completer.image_limits().map(|l| l.max_images), Some(1500));
        let images = [picture()];
        let answer = completer.complete_with_images(&request(&images)).await;
        assert_eq!(answer.as_deref(), Ok("two screens moved"));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        let parts = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(
            parts[0],
            json!({ "type": "text", "text": "stores at desk, staging" })
        );
        assert_eq!(parts[1]["type"], "image_url");
        let url = parts[1]["image_url"]["url"].as_str().unwrap();
        assert!(
            url.starts_with("data:image/png;base64,iVBORw0KGgo"),
            "{url}"
        );
        assert_eq!(parts[2]["text"], "Which changed on purpose?");
    }

    #[tokio::test]
    async fn a_refusal_of_the_pictures_and_a_failed_call_are_told_apart() {
        let refusal = json!({ "error": { "message": "this model does not take images" } });
        let server = backend(400, &refusal.to_string(), "application/json").await;
        let images = [picture()];
        let refused = completer(&server)
            .await
            .complete_with_images(&request(&images))
            .await;
        assert!(
            matches!(&refused, Err(OneShotImageError::ImagesNotSupported(why))
                if why.ends_with("this model does not take images")),
            "{refused:?}"
        );
        let server = backend(401, "no such key", "text/plain").await;
        let failed = completer(&server)
            .await
            .complete_with_images(&request(&images))
            .await;
        assert_eq!(
            failed,
            Err(OneShotImageError::Failed {
                class: "auth",
                message: "auth error: no such key".to_string(),
            })
        );
        // A refusal that does not say the model takes no images is a failed
        // call too, of the class a caller reads as "this request was refused".
        let too_long = json!({ "error": { "message": "prompt is too long" } });
        let server = backend(400, &too_long.to_string(), "application/json").await;
        let failed = completer(&server)
            .await
            .complete_with_images(&request(&images))
            .await;
        assert!(
            matches!(
                &failed,
                Err(OneShotImageError::Failed { class: "http", .. })
            ),
            "{failed:?}"
        );
    }

    #[test]
    fn every_failure_of_the_model_layer_has_its_class() {
        let class = |e: LlmError| match OneShotImageError::from(ImageCompletionError::Llm(e)) {
            OneShotImageError::Failed { class, .. } => class,
            other => panic!("{other:?}"),
        };
        assert_eq!(class(LlmError::Transient("503".into())), "transient");
        assert_eq!(class(LlmError::Parse("empty".into())), "parse");
        let limit = ImageCompletionError::LimitExceeded("9 pictures".into());
        assert_eq!(
            OneShotImageError::from(limit),
            OneShotImageError::LimitExceeded("9 pictures".into())
        );
    }
}
