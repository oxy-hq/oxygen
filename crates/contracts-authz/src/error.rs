//! The port's one failure: the facts behind an answer could not be read.

/// Why an enforcement read has no answer. Kept apart from "no": a caller that
/// must not read a failed lookup as a refusal (a queued send's delivery, say)
/// tells the two apart by this `Err`.
#[derive(Debug, thiserror::Error)]
pub enum AuthzError {
    /// The platform database could not answer. Carries the error unchanged, so
    /// a caller that returns `DbErr` gets back exactly the one it would have.
    #[error(transparent)]
    Unavailable(#[from] sea_orm::DbErr),
}

impl From<AuthzError> for sea_orm::DbErr {
    fn from(err: AuthzError) -> Self {
        match err {
            AuthzError::Unavailable(db) => db,
        }
    }
}
