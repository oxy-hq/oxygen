//! What `HEAD` names in a working copy, and the one label the API uses when it
//! names no branch.
//!
//! A detached HEAD has no branch, but every branch-shaped slot in the API
//! (`active_branch`, `?branch=`) still carries a value, so the server reports
//! `HEAD@<short sha>` there. That label is written and read in this module and
//! nowhere else — [`HeadState::label`] produces it, [`detached_label_sha`]
//! recognises it — so the producer and the recogniser cannot drift.
//!
//! The label is not a ref and is never handed to git. It means exactly one
//! thing: "the working copy as it is, on no branch". [`verify_detached_label`]
//! is what keeps a caller from using it for anything else.

use std::path::Path;

use oxy_shared::errors::OxyError;

use crate::cli::run;

/// Prefix of the label reported for a detached HEAD.
const DETACHED_PREFIX: &str = "HEAD@";

/// Bounds of the abbreviation `git rev-parse --short` can print: git never
/// abbreviates below 4 hex digits, and a full SHA-256 object id is 64.
const SHA_ABBREV_LEN: std::ops::RangeInclusive<usize> = 4..=64;

/// What `HEAD` names in a working copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadState {
    /// On a branch (an unborn one included — `git init` before a commit).
    Branch(String),
    /// On no branch: `HEAD` names a commit directly.
    Detached { short_sha: String },
}

impl HeadState {
    /// The value the API reports as the "current branch": the branch name, or
    /// `HEAD@<short sha>` when there is none.
    pub fn label(&self) -> String {
        match self {
            Self::Branch(name) => name.clone(),
            Self::Detached { short_sha } => format!("{DETACHED_PREFIX}{short_sha}"),
        }
    }

    /// The branch, or the refusal an operation that needs one answers with.
    pub fn into_branch(self) -> Result<String, DetachedHead> {
        match self {
            Self::Branch(name) => Ok(name),
            Self::Detached { short_sha } => Err(DetachedHead { short_sha }),
        }
    }
}

/// The working copy is on no branch, so an operation that needs one cannot run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("This workspace is on a detached HEAD at {short_sha}; switch to or create a branch first.")]
pub struct DetachedHead {
    pub short_sha: String,
}

impl From<DetachedHead> for OxyError {
    fn from(e: DetachedHead) -> Self {
        OxyError::RuntimeError(e.to_string())
    }
}

/// Reads what `HEAD` names in `root`.
pub async fn head_state(root: &Path) -> Result<HeadState, OxyError> {
    let out = run::run(root, &["branch", "--show-current"]).await?;
    let branch = out.trim();
    if !branch.is_empty() {
        return Ok(HeadState::Branch(branch.to_string()));
    }
    let sha = run::run(root, &["rev-parse", "--short", "HEAD"]).await?;
    Ok(HeadState::Detached {
        short_sha: sha.trim().to_string(),
    })
}

/// The sha inside a detached-HEAD label, or `None` for anything else.
///
/// Shape only, and exactly the shape [`HeadState::label`] emits: `HEAD@`
/// followed by a lowercase hex abbreviation of the length git prints. Nothing
/// that could be read as a ref, a path, or a revision expression matches.
pub fn detached_label_sha(label: &str) -> Option<&str> {
    let sha = label.strip_prefix(DETACHED_PREFIX)?;
    let is_hex = sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (SHA_ABBREV_LEN.contains(&sha.len()) && is_hex).then_some(sha)
}

/// Whether `label` has the shape of a detached-HEAD label. Says nothing about
/// whether any working copy is detached there — see [`verify_detached_label`].
pub fn is_detached_label(label: &str) -> bool {
    detached_label_sha(label).is_some()
}

/// The refusal for a request that names the detached label where a branch is
/// required (a switch target, a branch to pull, stage or compile). `None` for
/// any value that is not the label.
pub fn detached_label_refusal(value: &str) -> Option<DetachedHead> {
    detached_label_sha(value).map(|sha| DetachedHead {
        short_sha: sha.to_string(),
    })
}

/// Checks a detached-HEAD label against the working copy it claims to describe.
///
/// `Ok` only when `root` reports exactly this label right now: it is detached,
/// at that commit. A label for any other commit is refused — the checkout moved
/// since the caller read it, or the caller made it up — so the label can never
/// address anything but the working copy as it stands.
pub async fn verify_detached_label(root: &Path, label: &str) -> Result<(), OxyError> {
    let Some(sha) = detached_label_sha(label) else {
        return Err(OxyError::RuntimeError(format!(
            "'{label}' is not a detached HEAD label."
        )));
    };
    let current = head_state(root).await?;
    if current.label() == label {
        return Ok(());
    }
    Err(OxyError::RuntimeError(match current {
        HeadState::Branch(name) => format!(
            "This workspace is no longer on a detached HEAD at {sha}; it is on branch '{name}'."
        ),
        HeadState::Detached { short_sha } => format!(
            "This workspace is no longer on a detached HEAD at {sha}; it is detached at {short_sha}."
        ),
    }))
}

#[cfg(test)]
#[path = "head_tests.rs"]
mod tests;
