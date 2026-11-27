//! The model the notes are written with: the default model of the app's own
//! workspace, reached the way a queued task reaches a workspace's model
//! (`workspace_runtime::build_workspace_ctx`, then `agentic-pipeline`'s
//! prepared one-shot). Never another workspace's model, never the platform's.
//!
//! [`NoteModel`] is the seam the task is written against, so its tests hand it
//! a stub and no test calls a provider.
//!
//! Every call is `private`: the prompt quotes what an org's pages show, so
//! neither it nor the answer is put on the inference span, and a failure is
//! reported by its class, never by the provider's words.

use agentic_pipeline::OneShotCompleter;
use agentic_pipeline::one_shot::{ImageInput, ImageLimits, ImageRequest, OneShotImageError};
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// The label of the notes' calls on the inference span.
const AGENT_NAME: &str = "promote-checks-notes";

/// The most a note's answer may be, where the provider takes a cap. A note is
/// stored capped well under it (`notes`).
const MAX_ANSWER_TOKENS: u32 = 1_500;

/// Why the model did not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskError {
    /// The model's provider takes no pictures: it has no image input, its
    /// refusal of a request that carried some says so, or the request was
    /// past a bound it documents. The same words without pictures may be
    /// answered, and no later request need carry any.
    PicturesNotTaken,
    /// The call failed. The class of the failure (`auth`, `rate_limit`,
    /// `transient`, `http`, `parse`, ...): all that is kept of it.
    Failed(&'static str),
}

/// The class of a call the provider refused: a `4xx` that is neither an
/// authentication failure nor a rate limit (`agentic_llm`'s `LlmError::Http`,
/// as the one-shot names it).
pub const REFUSED: &str = "http";

impl AskError {
    /// Whether the provider refused this one request without saying it takes
    /// no pictures: a prompt too long with them, a picture it will not read.
    /// The same words without pictures may be answered; nothing says the next
    /// request's pictures will be refused.
    pub fn refused_this_request(self) -> bool {
        self == Self::Failed(REFUSED)
    }

    /// The failure in a note's words.
    pub fn said(self) -> String {
        match self {
            Self::PicturesNotTaken => "the model's provider did not take the pictures".to_string(),
            Self::Failed(class) => format!("the call to the model failed ({class})"),
        }
    }
}

/// What writes the notes.
#[async_trait]
pub trait NoteModel: Send + Sync {
    /// The model's name, as every note carries it.
    fn name(&self) -> &str;

    /// The bounds the model's provider documents for the pictures of one
    /// request; `None` when it takes none.
    fn image_limits(&self) -> Option<ImageLimits>;

    /// One answer to `system`, `images` (in order) and then `text`.
    async fn ask(
        &self,
        system: &str,
        text: &str,
        images: &[ImageInput],
    ) -> Result<String, AskError>;
}

/// A workspace's own model.
pub struct WorkspaceModel(OneShotCompleter);

/// How a failed one-shot is told to the task.
fn told(error: OneShotImageError) -> AskError {
    match error {
        OneShotImageError::ImagesNotSupported(_) | OneShotImageError::LimitExceeded(_) => {
            AskError::PicturesNotTaken
        }
        OneShotImageError::Failed { class, .. } => AskError::Failed(class),
    }
}

#[async_trait]
impl NoteModel for WorkspaceModel {
    fn name(&self) -> &str {
        self.0.model()
    }

    fn image_limits(&self) -> Option<ImageLimits> {
        self.0.image_limits()
    }

    async fn ask(
        &self,
        system: &str,
        text: &str,
        images: &[ImageInput],
    ) -> Result<String, AskError> {
        let request = ImageRequest {
            system,
            text,
            images,
            max_tokens: Some(MAX_ANSWER_TOKENS),
            private: true,
        };
        self.0.complete_with_images(&request).await.map_err(told)
    }
}

/// The default model of `workspace_id`, or why it has none this task can
/// use, in words fit for a row's summary: the workspace could not be loaded
/// on this node, or its configuration names no default model.
pub async fn of_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<WorkspaceModel, String> {
    let ctx = oxy_workspace_runtime::driver_ctx::build_workspace_ctx(workspace_id, db)
        .await
        .ok_or_else(|| "the app's workspace could not be loaded to read its model".to_string())?;
    let completer = agentic_pipeline::prepare_one_shot(&*ctx, None, AGENT_NAME)
        .await
        .map_err(|_| "the app's workspace has no default model configured".to_string())?;
    Ok(WorkspaceModel(completer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_of_the_pictures_and_a_failure_keep_only_their_kind() {
        let no_images = OneShotImageError::ImagesNotSupported("gpt-x takes no images".into());
        assert_eq!(told(no_images), AskError::PicturesNotTaken);
        let past = OneShotImageError::LimitExceeded("21 pictures".into());
        assert_eq!(told(past), AskError::PicturesNotTaken);
        let failed = OneShotImageError::Failed {
            class: "auth",
            message: "Incorrect API key provided: sk-live-1234".into(),
        };
        assert_eq!(told(failed), AskError::Failed("auth"));
        // The provider's words, a key included, are in neither.
        let said = AskError::Failed("auth").said();
        assert_eq!(said, "the call to the model failed (auth)");
    }

    /// A refused request is a failure of its own class: not the provider
    /// saying it takes no pictures, and not a failure of every request.
    #[test]
    fn a_refused_request_is_told_apart_from_no_image_input_and_from_other_failures() {
        let refused = OneShotImageError::Failed {
            class: "http",
            message: "prompt is too long".into(),
        };
        assert_eq!(told(refused), AskError::Failed(REFUSED));
        assert!(AskError::Failed(REFUSED).refused_this_request());
        for other in [
            AskError::PicturesNotTaken,
            AskError::Failed("auth"),
            AskError::Failed("rate_limit"),
            AskError::Failed("transient"),
        ] {
            assert!(!other.refused_this_request(), "{other:?}");
        }
    }
}
