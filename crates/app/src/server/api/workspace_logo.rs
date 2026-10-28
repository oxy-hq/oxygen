//! `GET /api/{workspace_id}/logo` — workspace branding.
//!
//! The org-level **uploaded** logo (Settings → General), stored on the
//! organization row — see `org_logo`. White-labels the HQ chrome. With none
//! uploaded the answer is 404 and the frontend renders the name initial.
//!
//! A `logo.{svg,png,...}` file at the workspace root used to be a second
//! source. It is gone: it could be read only on the process that held a
//! working copy, so the same URL answered differently from one replica to
//! the next.
//!
//! SVG logos can carry inline `<script>`, so every logo is served through
//! `logo_response`, which attaches download + sandbox headers that neutralize
//! script execution on direct navigation without affecting the `<img>` chrome.

use axum::extract::Path as AxumPath;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use entity::{organizations, workspaces};
use oxy::database::client::establish_connection;
use sea_orm::EntityTrait;
use uuid::Uuid;

/// Build a logo response with stored-XSS hardening headers.
///
/// An SVG logo (`image/svg+xml`) can embed `<script>` that executes if the URL
/// is opened as a **top-level document**. The org-uploaded logo (which an
/// org admin controls) flows through here, so the vector is neutralized at
/// this single serving boundary:
///
/// - `Content-Disposition: attachment` — a direct navigation downloads the
///   bytes instead of rendering them as a document.
/// - `Content-Security-Policy: default-src 'none'; sandbox` — even if rendered,
///   the document is sandboxed (unique origin, scripts disabled) and may load
///   no subresources.
/// - `X-Content-Type-Options: nosniff` — honor our `Content-Type` instead of
///   sniffing the bytes into an executable type.
///
/// None of these affect the legitimate `<img>` embedding in the rail/heading:
/// `<img>` ignores `Content-Disposition`, renders SVG in script-free secure
/// mode, and is not governed by the response's own CSP.
// `pub` for `oxy-api-tenancy`'s org logo, which serves bytes the same way —
// on its `surface_contract` backlog.
pub fn logo_response(content_type: impl Into<String>, bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type.into()),
            // Branding changes anytime (git pull or re-upload) — revalidate.
            // The frontend's `?v=updated_at` busts the already-rendered <img>.
            (header::CACHE_CONTROL, "no-cache".to_string()),
            (header::CONTENT_DISPOSITION, "attachment".to_string()),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; sandbox".to_string(),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        bytes,
    )
        .into_response()
}

/// The org-level uploaded logo for the workspace's org, if any. Two small
/// lookups (workspace → org_id → org); returns `None` in local mode (nil
/// workspace, no org row) or when no logo has been uploaded.
async fn uploaded_org_logo(workspace_id: Uuid) -> Option<Response> {
    let db = establish_connection().await.ok()?;
    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .ok()??;
    let org = organizations::Entity::find_by_id(ws.org_id?)
        .one(&db)
        .await
        .ok()??;
    let bytes = org.logo?;
    let mime = org
        .logo_content_type
        .unwrap_or_else(|| "image/png".to_string());
    Some(logo_response(mime, bytes))
}

pub async fn get_workspace_logo(
    AxumPath(workspace_id): AxumPath<Uuid>,
) -> Result<Response, StatusCode> {
    uploaded_org_logo(workspace_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_response_carries_xss_hardening_headers() {
        // An SVG logo is attacker-controllable (org-admin upload); the serve
        // boundary must neutralize inline-script execution.
        let resp = logo_response("image/svg+xml", b"<svg/>".to_vec());
        let h = resp.headers();
        assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "image/svg+xml");
        assert_eq!(h.get(header::CONTENT_DISPOSITION).unwrap(), "attachment");
        assert_eq!(
            h.get(header::CONTENT_SECURITY_POLICY).unwrap(),
            "default-src 'none'; sandbox"
        );
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(h.get(header::CACHE_CONTROL).unwrap(), "no-cache");
    }
}
