//! Which requests are served from a preview, and the headers that say so.
//!
//! A preview request carries `x-oxy-preview-revision: <revision_id>`. The
//! frontend also sends `?branch=<label>`, but only for its own cache keys:
//! `?branch=` alone is what the IDE sends while someone edits that branch, and
//! that request must keep reading the working copy. So the header is the only
//! switch, and everything here is a no-op without it.
//!
//! With it, the request is pinned to the revision only when all three hold:
//! the caller may preview (`WorkspacePreviewer`, staff); the revision is a
//! ready `staging`/`main` revision of THIS workspace whose compiled config the
//! runtime can load (`custom_apps_staging_pin::check_pinnable`); and a **live
//! preview** of this workspace is at the revision's commit
//! (`store::previewed_at`). The last is what makes deleting a preview, or
//! refreshing it past a commit, end that revision's life as a preview — even
//! when the revision itself survives because an app build pins it or a run
//! still reads it. Any miss leaves the request exactly as it would have been
//! without the header.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request};
use axum::response::Response;
use uuid::Uuid;

/// Request header that marks a preview request: the revision to read.
pub const REQUEST_HEADER: &str = "x-oxy-preview-revision";

/// Response header on every response served from a preview:
/// `<branch>@<revision_id>`.
pub const RESPONSE_HEADER: &str = "x-oxy-preview";

/// Set by `enforce_role` on a serve replica that kept a `?branch=` request on
/// the fleet because it carried the preview header, instead of escalating it
/// to the ide. The workspace middleware finishes the decision once it knows
/// the caller: if the preview does not apply after all, the request goes to
/// the ide exactly as it would have without the header.
#[derive(Clone, Copy, Debug)]
pub struct DeferredBranchEscalation;

/// A request pinned to a preview revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewPin {
    /// The label the response names: the request's `?branch=`, else the branch
    /// the revision was compiled from.
    pub branch: String,
    pub revision_id: Uuid,
}

impl PreviewPin {
    pub fn header_value(&self) -> String {
        format!("{}@{}", self.branch, self.revision_id)
    }

    /// Mark `response` as served from this preview. The branch is
    /// percent-encoded when it is not a legal header value as it stands.
    pub fn stamp(&self, response: &mut Response) {
        let value = HeaderValue::from_str(&self.header_value()).or_else(|_| {
            let encoded = format!("{}@{}", urlencoding::encode(&self.branch), self.revision_id);
            HeaderValue::from_str(&encoded)
        });
        if let Ok(v) = value {
            response.headers_mut().insert(RESPONSE_HEADER, v);
        }
    }
}

/// The revision a request asks to preview, or `None` for an ordinary request
/// (no header, or one that is not a revision id — ignored, not refused).
pub fn requested_revision(headers: &HeaderMap) -> Option<Uuid> {
    let raw = headers.get(REQUEST_HEADER)?.to_str().ok()?;
    Uuid::parse_str(raw.trim()).ok()
}

/// Whether the raw request carries the preview header at all — the cheap check
/// `enforce_role` makes before any database or identity is available.
pub fn has_preview_header(headers: &HeaderMap) -> bool {
    headers.contains_key(REQUEST_HEADER)
}

/// The pin for this request, if it asked for one and may have it.
///
/// Takes the request by value because the staff check reads (and memoizes into)
/// its extensions; hands it back either way.
pub async fn for_request(
    workspace_id: Uuid,
    query_branch: Option<&str>,
    request: Request<Body>,
) -> (Request<Body>, Option<PreviewPin>) {
    let Some(revision_id) = requested_revision(request.headers()) else {
        return (request, None);
    };
    let (mut parts, body) = request.into_parts();
    let may_preview = oxy_server_authz::role_guards::may_preview(&mut parts).await;
    let request = Request::from_parts(parts, body);
    if !may_preview {
        tracing::debug!(%workspace_id, %revision_id, "preview header from a caller who may not preview; ignoring it");
        return (request, None);
    }
    let pin = servable_pin(workspace_id, revision_id, query_branch).await;
    (request, pin)
}

/// The pin, when the revision is one this workspace may serve
/// (`check_pinnable`: ready, `staging`/`main`, this workspace's, config loads)
/// and a live preview is at its commit.
async fn servable_pin(
    workspace_id: Uuid,
    revision_id: Uuid,
    query_branch: Option<&str>,
) -> Option<PreviewPin> {
    let db = oxy::database::client::establish_connection().await.ok()?;
    let rev = match crate::server::api::custom_apps_staging_pin::check_pinnable(
        &db,
        workspace_id,
        revision_id,
    )
    .await
    {
        Ok(rev) => rev,
        Err(refusal) => {
            tracing::debug!(%workspace_id, %revision_id, %refusal, "preview header names a revision this workspace cannot serve; ignoring it");
            return None;
        }
    };
    match super::store::previewed_at(&db, workspace_id, &rev.git_sha).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(%workspace_id, %revision_id, "preview header names a revision no live preview is at (deleted, or refreshed past); ignoring it");
            return None;
        }
        Err(error) => {
            tracing::warn!(%workspace_id, %revision_id, %error, "previews: could not read the preview registry; ignoring the preview header");
            return None;
        }
    }
    let branch = query_branch
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .or(rev.branch)
        .unwrap_or_default();
    Some(PreviewPin {
        branch,
        revision_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(REQUEST_HEADER, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn no_header_is_an_ordinary_request() {
        assert_eq!(requested_revision(&HeaderMap::new()), None);
        assert!(!has_preview_header(&HeaderMap::new()));
    }

    #[test]
    fn the_header_names_a_revision_id_and_nothing_else() {
        let id = Uuid::new_v4();
        assert_eq!(requested_revision(&headers(&id.to_string())), Some(id));
        assert_eq!(
            requested_revision(&headers(&format!(" {id} "))),
            Some(id),
            "surrounding whitespace is not a different revision"
        );
        for junk in ["feat/x", "", "latest", "0"] {
            assert_eq!(requested_revision(&headers(junk)), None, "{junk:?}");
        }
    }

    #[test]
    fn the_response_header_names_branch_and_revision() {
        let pin = PreviewPin {
            branch: "feat/x".into(),
            revision_id: Uuid::nil(),
        };
        let mut resp = Response::new(Body::empty());
        pin.stamp(&mut resp);
        assert_eq!(
            resp.headers().get(RESPONSE_HEADER).unwrap(),
            "feat/x@00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn a_branch_that_is_not_a_legal_header_value_is_percent_encoded() {
        let pin = PreviewPin {
            branch: "feat/\u{1}bell".into(),
            revision_id: Uuid::nil(),
        };
        let mut resp = Response::new(Body::empty());
        pin.stamp(&mut resp);
        assert_eq!(
            resp.headers().get(RESPONSE_HEADER).unwrap(),
            "feat%2F%01bell@00000000-0000-0000-0000-000000000000"
        );
    }
}
