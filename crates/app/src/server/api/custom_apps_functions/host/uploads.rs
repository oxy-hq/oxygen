//! The upload URLs one invocation minted, and the `ctx.fetch` decision that
//! reads them.
//!
//! Outside production a mutating `ctx.fetch` is held — except a PUT to an
//! upload URL this invocation's own `ctx.storage.getUploadUrl` minted into
//! its environment's silo (`env_policy::upload`). The decision is the
//! policy's; this is only what it is decided from: the host keeps each URL
//! the storage module answered, so a request is matched against what the host
//! itself signed and never against anything the function says about it.
//!
//! An allowed upload is recorded as an isolated storage write is — it runs,
//! under its host-call span (`http.client.request`: method, host, status),
//! with one `host call isolated` event — and is **not** noted in the
//! held-write log: it is not a held write.

use std::sync::{Mutex, PoisonError};

use super::super::env_policy::{Decision, HostOp, MintedUpload, Target};
use super::*;
use crate::server::api::custom_apps_storage::{Silo, UploadUrl};

/// How many minted uploads one invocation keeps. Past it the oldest is
/// forgotten, so a run that mints and uploads in a loop is never affected,
/// and a PUT to a forgotten URL is held like any other — the list fails
/// closed and stays bounded however many URLs a run mints.
const MAX_MINTED_UPLOADS: usize = 1024;

/// The uploads this invocation minted, oldest first.
#[derive(Default)]
pub(super) struct MintedUploads(Mutex<Vec<MintedUpload>>);

impl MintedUploads {
    fn note(&self, upload: MintedUpload) {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.len() >= MAX_MINTED_UPLOADS {
            kept.remove(0);
        }
        kept.push(upload);
    }

    /// `decide` over the uploads kept so far. Never held across an await.
    fn decide(&self, decide: impl FnOnce(&[MintedUpload]) -> Decision) -> Decision {
        decide(&self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl ProjectFunctionHost {
    /// Keep the upload URL `ctx.storage.getUploadUrl` just minted into
    /// `silo`. Production's is not kept: production allows every fetch and
    /// never asks.
    pub(super) fn note_minted_upload(&self, silo: &Silo, minted: &UploadUrl) {
        if silo.is_production() {
            return;
        }
        match MintedUpload::new(&minted.url, &minted.key) {
            Some(upload) => self.minted_uploads.note(upload),
            None => tracing::warn!(
                environment = %self.policy.environment(),
                "a minted upload URL does not parse; a ctx.fetch PUT to it will be held"
            ),
        }
    }

    /// What the policy decides for a mutating `ctx.fetch` of `method` to
    /// `url` (`EnvPolicy::decide_on_fetch`), given what this invocation
    /// minted.
    pub(super) fn decide_fetch(&self, method: &str, url: &reqwest::Url) -> Decision {
        let decided = self.minted_uploads.decide(|minted| {
            self.policy
                .decide_on_fetch(self.app_id, method, url, minted)
        });
        if decided == Decision::Isolate(Target::StorageSilo) {
            tracing::info!(
                op = HostOp::Fetch.name(),
                environment = %self.policy.environment(),
                home = "storage_silo",
                "host call isolated: an upload to the environment's own storage silo"
            );
        }
        decided
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(n: usize) -> MintedUpload {
        MintedUpload::new(&format!("https://objects.example.com/b/k{n}?sig=1"), "k").expect("a URL")
    }

    fn kept(uploads: &MintedUploads) -> Vec<MintedUpload> {
        let mut seen = Vec::new();
        uploads.decide(|minted| {
            seen = minted.to_vec();
            Decision::Hold
        });
        seen
    }

    #[test]
    fn the_list_is_bounded_and_forgets_the_oldest() {
        let uploads = MintedUploads::default();
        assert!(kept(&uploads).is_empty());
        for n in 0..MAX_MINTED_UPLOADS + 2 {
            uploads.note(upload(n));
        }
        let kept = kept(&uploads);
        assert_eq!(kept.len(), MAX_MINTED_UPLOADS);
        assert_eq!(kept.first(), Some(&upload(2)), "the two oldest are gone");
        assert_eq!(kept.last(), Some(&upload(MAX_MINTED_UPLOADS + 1)));
    }

    /// The client `ctx.fetch` sends with follows no redirect, so an upload
    /// the policy sends to a minted URL cannot be bounced to another key,
    /// bucket or host: the function gets the 3xx back, and a PUT to its
    /// `Location` is a new fetch, decided on its own (and not on the list).
    #[tokio::test]
    async fn a_redirected_upload_is_answered_to_the_function_and_never_followed() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let store = MockServer::start().await;
        let elsewhere = format!("{}/bucket/another-silo/key", store.uri());
        for status in [301u16, 302, 303, 307, 308] {
            Mock::given(method("PUT"))
                .and(path(format!("/bucket/silo/{status}")))
                .respond_with(
                    ResponseTemplate::new(status).insert_header("location", elsewhere.as_str()),
                )
                .mount(&store)
                .await;
        }
        Mock::given(path("/bucket/another-silo/key"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&store)
            .await;

        let client = fetch_client();
        for status in [301u16, 302, 303, 307, 308] {
            let answered = client
                .put(format!("{}/bucket/silo/{status}", store.uri()))
                .body("hello")
                .send()
                .await
                .expect("the store answers");
            assert_eq!(answered.status().as_u16(), status);
        }
        let reached: Vec<String> = store
            .received_requests()
            .await
            .expect("recorded requests")
            .iter()
            .map(|r| r.url.path().to_string())
            .collect();
        assert_eq!(
            reached,
            [301u16, 302, 303, 307, 308].map(|s| format!("/bucket/silo/{s}")),
            "nothing reached the redirect's target"
        );
    }
}
