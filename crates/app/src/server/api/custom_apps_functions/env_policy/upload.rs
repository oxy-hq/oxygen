//! The one mutating `ctx.fetch` a non-production run sends: an upload into
//! its environment's own storage silo.
//!
//! `ctx.storage.getUploadUrl` outside production mints a presigned PUT into
//! the environment's silo ([`Target::StorageSilo`]). A function that then
//! uploads the bytes itself does it through `ctx.fetch` — which holds every
//! mutating request, so an app that uploads from a function could not be
//! exercised in staging or in a sandbox. That PUT writes exactly the object
//! `ctx.storage.put` would have written: the same silo, under the same cap
//! (admitted when the URL was minted, for the length bound into its
//! signature) and the same 30-day expiry.
//!
//! **What is verified, and from what.** Nothing the function asserts. The
//! host keeps every upload URL its own `getUploadUrl` minted for this
//! invocation ([`MintedUpload`]), and [`EnvPolicy::decide_on_fetch`] sends a
//! fetch only when all of these hold:
//!
//! - its method is `PUT` — the one method the signer issues an upload for
//!   (`custom_apps_storage::s3::presign_put`; no POST policy is ever minted);
//! - its URL **is** one of those, compared whole and as parsed, which is the
//!   value the request is sent to. There is no endpoint, bucket or prefix
//!   pattern to get around: another host or port, another bucket, a `..`
//!   segment (plain or encoded), a query or fragment added or dropped, a URL
//!   into production's silo, another environment's or another app's, and one
//!   an earlier invocation minted are all simply not on the list;
//! - the key that URL signs lies in the silo this policy isolates
//!   `getUploadUrl` to for this app ([`EnvPolicy::silo_for`]) — so a record
//!   naming any other silo would not pass even if one were ever kept.
//!
//! Everything else is decided by the op alone, which holds it. The host's
//! fetch client follows no redirect (`host::fetch_client`), so the bytes go to
//! the minted URL or nowhere.
//!
//! Production is `Allow`, as for every op, and never reads the list. A
//! production run reading a branch has no silo of its own: its `getUploadUrl`
//! is held, nothing is minted for it, and its fetch holds.

use uuid::Uuid;

use super::{Decision, EnvPolicy, HostOp, Target};

/// The method an upload URL is signed for.
const UPLOAD_METHOD: &str = "PUT";

/// An upload URL this invocation's `ctx.storage.getUploadUrl` minted, and the
/// silo key it signs. Built by the host from what the storage module answered
/// (`host::uploads`), never from anything the function sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintedUpload {
    url: url::Url,
    key: String,
}

impl MintedUpload {
    /// `None` when `url` does not parse: such a URL could never equal a
    /// request's, which is parsed the same way before it is decided.
    pub fn new(url: &str, key: &str) -> Option<Self> {
        Some(Self {
            url: url::Url::parse(url).ok()?,
            key: key.to_string(),
        })
    }
}

impl EnvPolicy {
    /// [`Self::decide`] for a mutating `ctx.fetch` of `method` to `url`, given
    /// the uploads this invocation minted: a request the op alone holds is
    /// `Isolate(Target::StorageSilo)` when it is an upload into this
    /// environment's own silo ([`Self::is_own_upload`]). The only decision
    /// that turns a `Hold` into an isolated write; it never answers `Allow`
    /// outside production.
    pub fn decide_on_fetch(
        &self,
        app_id: Uuid,
        method: &str,
        url: &url::Url,
        minted: &[MintedUpload],
    ) -> Decision {
        match self.decide(HostOp::Fetch) {
            Decision::Hold if self.is_own_upload(app_id, method, url, minted) => {
                Decision::Isolate(Target::StorageSilo)
            }
            decided => decided,
        }
    }

    /// Is a `method` request to `url` an upload this invocation minted into
    /// the environment's own silo of `app_id`? See the module docs for each
    /// condition.
    fn is_own_upload(
        &self,
        app_id: Uuid,
        method: &str,
        url: &url::Url,
        minted: &[MintedUpload],
    ) -> bool {
        if !method.eq_ignore_ascii_case(UPLOAD_METHOD) {
            return false;
        }
        // The silo `getUploadUrl` itself works in here. None when that op is
        // held or refused; production's is never an environment's own.
        let Some(silo) = self
            .silo_for(HostOp::StorageGetUploadUrl, app_id)
            .filter(|silo| !silo.is_production())
        else {
            return false;
        };
        let prefix = silo.prefix();
        minted
            .iter()
            .any(|upload| upload.url == *url && upload.key.starts_with(&prefix))
    }
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
