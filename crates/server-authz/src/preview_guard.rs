//! Who may preview a workspace's branch: the [`WorkspacePreviewer`] decision
//! over the request's extensions.
//!
//! It reads nothing of the request but its extensions, so it takes them rather
//! than axum's `Parts`: `role_guards::may_preview` (the `Parts` form) and the
//! enforcement port's `may_preview` both delegate here. Moved out of
//! `role_guards.rs` with its body unchanged (C3a, 2026-11-28).
//!
//! [`WorkspacePreviewer`]: crate::role_guards::WorkspacePreviewer

use axum::http::Extensions;

/// `existing_allow` is the shipped staff door — what the `/admin` console grants
/// today (`OXY_OWNER` or a platform-grant row, any role). The ring then subtracts
/// what that door is too coarse to see: an App Operator (apps and nothing else)
/// and a grant whose scope does not reach this org. A workspace with no org has
/// no tenant to be staff *in*, so it is never previewable.
pub async fn may_preview_in(extensions: &mut Extensions) -> bool {
    let Some(ws) = extensions.get::<entity::workspaces::Model>().cloned() else {
        return false;
    };
    let Some(org_id) = ws.org_id else {
        return false;
    };
    let Some(caller) = crate::Caller::from_extensions(extensions) else {
        return false;
    };
    let legacy = match oxy_platform::db::establish_connection().await {
        Ok(db) => crate::globals::platform_standing(&db, &caller)
            .await
            .is_staff(),
        // No database, no standing we can vouch for: previews fail closed.
        Err(_) => false,
    };
    crate::enforce_guard_in(
        extensions,
        "guard.workspace_previewer",
        crate::Action::WorkspacePreview,
        crate::Resource::workspace(ws.id, org_id),
        legacy,
    )
    .await
}
