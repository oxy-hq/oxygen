//! `GET /api/{workspace_id}/logo` — workspace branding.
//!
//! Resolution order:
//!   1. The org-level **uploaded** logo (Settings → General), stored on the
//!      organization row — see `org_logo`. White-labels the HQ chrome.
//!   2. The code-first `logo.{svg,png,jpg,jpeg,webp}` file (in that
//!      precedence order) at the workspace root, beside `config.yml`.
//!   3. 404 (the frontend then renders the name initial).
//!
//! The file candidate list is fixed — no user input ever reaches the
//! filesystem path, so there is no traversal surface.
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

use crate::server::api::middlewares::workspace_context::WorkspaceManagerReadOnly;

/// Build a logo response with stored-XSS hardening headers.
///
/// An SVG logo (`image/svg+xml`) can embed `<script>` that executes if the URL
/// is opened as a **top-level document**. Both the org-uploaded logo (which an
/// org admin controls) and the code-first `logo.svg` flow through here, so the
/// vector is neutralized at this single serving boundary:
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
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Response, StatusCode> {
    // 1. Org-level uploaded logo wins (white-label).
    if let Some(resp) = uploaded_org_logo(workspace_id).await {
        return Ok(resp);
    }
    // 2. Fall back to the code-first file at the workspace root.
    //
    // This route stays `FleetOk` on purpose even though the read is FS-bound:
    // the logo loads on every page, so proxying it to the ide would put the
    // whole product's chrome behind the singleton. The cost is that a
    // code-first logo appears on the ide and not on a replica, and the caller
    // gets a clean 404 that the frontend already renders as a monogram.
    //
    // The durable fix is compiling the logo like every other workspace artifact
    // so it is served from the boundary. Until then this is a known, bounded
    // inconsistency rather than a silent one.
    //
    // Both absences are a 404 to the caller, and they are logged apart:
    // `Ok(None)` is this workspace having no logo, `Err` is this NODE having no
    // files. Only the first is a fact about the customer.
    let path_and_mime = match workspace_manager.config_manager.workspace_logo().await {
        Ok(found) => found,
        Err(e) => {
            tracing::debug!(error = %e, "workspace logo: no source on this node");
            None
        }
    };
    let Some((path, mime)) = path_and_mime else {
        return Err(StatusCode::NOT_FOUND);
    };
    let bytes = tokio::fs::read(&path).await.map_err(|e| {
        tracing::warn!("workspace logo read failed at {}: {e}", path.display());
        StatusCode::NOT_FOUND
    })?;
    Ok(logo_response(mime, bytes))
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
