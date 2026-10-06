//! Typed role-guard extractors. Each guard reads an upstream extension
//! populated by `org_middleware` / `workspace_middleware` /
//! `local_context_middleware` and fails extraction with `403 FORBIDDEN`
//! if the caller's role is insufficient.
//!
//! Using a guard type as a handler parameter is the check — it cannot be
//! forgotten. Handlers that need the full context can destructure:
//!
//! ```ignore
//! pub async fn delete_org(OrgOwner(ctx): OrgOwner) { ... }
//! pub async fn force_push(_: WorkspaceAdmin) { ... }
//! ```
//!
//! The guards assume the relevant middleware has already inserted the
//! extension; missing extensions yield `500` (a wiring bug, not a caller
//! error).

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use entity::org_members::OrgRole;
use entity::workspace_members::WorkspaceRole;
use std::future::Future;
use std::marker::PhantomData;

use crate::org_context::OrgContext;
use crate::workspace_role::EffectiveWorkspaceRole;
// The guards call `authz::enforce_guard(..)` / `authz::Action` / `authz::Resource`.
// In `oxy-app` `authz` was `crate::server::authz`; here that surface IS this crate's
// root (`enforce_guard` is defined in lib.rs; `Action`/`Resource` come from the
// `pub use oxy_authz::*`). Aliasing keeps every call site below byte-identical.
use crate as authz;

/// Caller is the Org Owner. Only Owners pass; Admins and Members are rejected.
#[derive(Debug)]
pub struct OrgOwner(pub OrgContext);

impl<S> FromRequestParts<S> for OrgOwner
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let Some(ctx) = parts.extensions.get::<OrgContext>().cloned() else {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            };
            let legacy = ctx.membership.role == OrgRole::Owner;
            let allowed = authz::enforce_guard(
                parts,
                "guard.org_owner",
                authz::Action::OrgOwnerManage,
                authz::Resource::org(ctx.org.id),
                legacy,
            )
            .await;
            if allowed {
                Ok(OrgOwner(ctx))
            } else {
                Err(StatusCode::FORBIDDEN)
            }
        }
    }
}

/// Caller is an Org Owner or Admin.
#[derive(Debug)]
pub struct OrgAdmin(pub OrgContext);

impl<S> FromRequestParts<S> for OrgAdmin
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // Enforce the whole OrgAdmin ring at its choke point — MemberSetRole is a
        // representative action of that ring.
        async move {
            org_admin(parts, "guard.org_admin", authz::Action::MemberSetRole)
                .await
                .map(OrgAdmin)
        }
    }
}

/// The org-admin choke point for one named action: the shipped check (the
/// request's org role is Owner or Admin) AND the model's verdict on `action`.
async fn org_admin(
    parts: &mut Parts,
    label: &'static str,
    action: authz::Action,
) -> Result<OrgContext, StatusCode> {
    let Some(ctx) = parts.extensions.get::<OrgContext>().cloned() else {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let legacy = matches!(ctx.membership.role, OrgRole::Owner | OrgRole::Admin);
    let resource = authz::Resource::org(ctx.org.id);
    if authz::enforce_guard(parts, label, action, resource, legacy).await {
        Ok(ctx)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// An action that sits on the org-admin ring and wants to be **named** in the
/// decision, rather than riding [`OrgAdmin`]'s representative `MemberSetRole`.
/// The differential tests pin each such action to this guard's oracle.
pub trait OrgAdminAction {
    /// The `authz` log label of the guard.
    const LABEL: &'static str;
    const ACTION: authz::Action;
}

/// Caller is an Org Owner or Admin, decided on `A`'s own action. Same door as
/// [`OrgAdmin`] — the same shipped check beside the model — so a surface gets
/// its own line in the `authz` log and its own differential case without a
/// hand-written role match.
pub struct OrgAdminFor<A>(pub OrgContext, pub PhantomData<A>);

impl<S, A> FromRequestParts<S> for OrgAdminFor<A>
where
    S: Send + Sync,
    A: OrgAdminAction,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            org_admin(parts, A::LABEL, A::ACTION)
                .await
                .map(|ctx| OrgAdminFor(ctx, PhantomData))
        }
    }
}

/// Caller is an Org Admin **on this org for real** — Global Owner / Global
/// Admin synthetic-Owner memberships are explicitly rejected. Use for
/// surfaces that must never be exposed via the cross-tenant operator
/// fallback in [`super::org_context`] — billing (Stripe portal, invoices,
/// checkout), per-org admin-promotion changes, anything else where the
/// distinction between "Oxy operator" and "tenant officer" matters.
///
/// The reasoning is captured in `product-context.md` under
/// "Roles & permissions": Global Admins are intentionally barred from
/// billing and from the admin promotion/demotion surface. Most tenant
/// admin actions remain reachable through `OrgAdmin`.
#[derive(Debug)]
pub struct OrgAdminStrict(pub OrgContext);

impl<S> FromRequestParts<S> for OrgAdminStrict
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let Some(ctx) = parts.extensions.get::<OrgContext>().cloned() else {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            };
            // Real owner/admin only — the global-operator override is rejected.
            let legacy = !ctx.is_global_override
                && matches!(ctx.membership.role, OrgRole::Owner | OrgRole::Admin);
            // OrgBilling is the OrgAdminStrict ring (admin_orgs only, no override).
            let allowed = authz::enforce_guard(
                parts,
                "guard.org_admin_strict",
                authz::Action::OrgBilling,
                authz::Resource::org(ctx.org.id),
                legacy,
            )
            .await;
            if allowed {
                Ok(OrgAdminStrict(ctx))
            } else {
                Err(StatusCode::FORBIDDEN)
            }
        }
    }
}

/// Same shape as [`super::org_context::OrgContextExtractor`] but rejects the
/// synthetic-Owner override path. Use when a member-readable surface must
/// not be visible across tenants via the operator fallback — currently the
/// billing-status banner and the checkout-session verifier. Pair with
/// [`OrgAdminStrict`] for mutating billing routes.
#[derive(Debug)]
pub struct OrgMemberStrict(pub OrgContext);

impl<S> FromRequestParts<S> for OrgMemberStrict
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let Some(ctx) = parts.extensions.get::<OrgContext>().cloned() else {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            };
            // Any real member — the cross-tenant override is rejected.
            let legacy = !ctx.is_global_override;
            let allowed = authz::enforce_guard(
                parts,
                "guard.org_member_strict",
                authz::Action::OrgReadStrict,
                authz::Resource::org(ctx.org.id),
                legacy,
            )
            .await;
            if allowed {
                Ok(OrgMemberStrict(ctx))
            } else {
                Err(StatusCode::FORBIDDEN)
            }
        }
    }
}

/// Caller's effective workspace role is Owner or Admin.
/// Use for destructive or settings-changing workspace actions.
#[derive(Debug)]
pub struct WorkspaceAdmin(pub WorkspaceRole);

impl<S> FromRequestParts<S> for WorkspaceAdmin
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let Some(EffectiveWorkspaceRole(role)) =
                parts.extensions.get::<EffectiveWorkspaceRole>().cloned()
            else {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            };
            let legacy = matches!(role, WorkspaceRole::Owner | WorkspaceRole::Admin);
            // Enforce the WorkspaceManage ring; the workspace's org (from the Model the
            // middleware attached) lets the org->workspace hierarchy resolve. When the
            // workspace has no org (single-workspace local mode) the org model doesn't
            // apply, so the legacy verdict stands.
            let allowed = match parts.extensions.get::<entity::workspaces::Model>().cloned() {
                Some(ws) => match ws.org_id {
                    Some(org_id) => {
                        authz::enforce_guard(
                            parts,
                            "guard.workspace_admin",
                            authz::Action::WorkspaceManage,
                            authz::Resource::workspace(ws.id, org_id),
                            legacy,
                        )
                        .await
                    }
                    None => legacy,
                },
                None => legacy,
            };
            if allowed {
                Ok(WorkspaceAdmin(role))
            } else {
                Err(StatusCode::FORBIDDEN)
            }
        }
    }
}

/// Permission helper for workspace-rename: callers must be Org Owner/Admin
/// OR the user that originally created the workspace. The rule depends on
/// workspace data not present in request extensions, so it can't be a pure
/// typed extractor — keeping it here preserves the "all role checks live
/// in role_guards" convention so future readers can grep one place.
pub fn ensure_org_admin_or_workspace_creator(
    ctx: &OrgContext,
    workspace: &entity::workspaces::Model,
) -> Result<(), (StatusCode, String)> {
    let is_admin = matches!(ctx.membership.role, OrgRole::Owner | OrgRole::Admin);
    let is_creator = workspace.created_by == Some(ctx.membership.user_id);
    if !is_admin && !is_creator {
        return Err((
            StatusCode::FORBIDDEN,
            "Only admins or workspace creator can rename".to_string(),
        ));
    }
    Ok(())
}

/// Caller can edit workspace contents (Owner/Admin/Member). Rejects Viewer.
/// Use for contributor actions: commit, push, pull, file edit.
#[derive(Debug)]
pub struct WorkspaceEditor(pub WorkspaceRole);

impl<S> FromRequestParts<S> for WorkspaceEditor
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let Some(EffectiveWorkspaceRole(role)) =
                parts.extensions.get::<EffectiveWorkspaceRole>().cloned()
            else {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            };
            let legacy = role > WorkspaceRole::Viewer;
            let allowed = match parts.extensions.get::<entity::workspaces::Model>().cloned() {
                Some(ws) => match ws.org_id {
                    Some(org_id) => {
                        authz::enforce_guard(
                            parts,
                            "guard.workspace_editor",
                            authz::Action::WorkspaceEdit,
                            authz::Resource::workspace(ws.id, org_id),
                            legacy,
                        )
                        .await
                    }
                    None => legacy,
                },
                None => legacy,
            };
            if allowed {
                Ok(WorkspaceEditor(role))
            } else {
                Err(StatusCode::FORBIDDEN)
            }
        }
    }
}

/// Caller may create, refresh, delete or open a workspace **preview**: Oxy staff
/// whose grant carries `OperatePlatform` over the workspace's org
/// (`Action::WorkspacePreview`). Customers never do — they don't build or change
/// workspaces; Oxy does.
///
/// Runs behind the workspace middleware, so the caller already reached this
/// workspace (a real membership, or staff with a live assume session); this adds
/// the staff half on top.
#[derive(Debug)]
pub struct WorkspacePreviewer;

impl<S> FromRequestParts<S> for WorkspacePreviewer
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            if parts
                .extensions
                .get::<entity::workspaces::Model>()
                .is_none()
            {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            if may_preview(parts).await {
                Ok(WorkspacePreviewer)
            } else {
                Err(StatusCode::FORBIDDEN)
            }
        }
    }
}

/// The [`WorkspacePreviewer`] decision as a boolean, for the request middleware
/// that decides whether to honour a `?branch=` preview pin. One function, so the
/// API and the pin can never disagree about who may preview.
///
/// `existing_allow` is the shipped staff door — what the `/admin` console grants
/// today (`OXY_OWNER` or a platform-grant row, any role). The ring then subtracts
/// what that door is too coarse to see: an App Operator (apps and nothing else)
/// and a grant whose scope does not reach this org. A workspace with no org has
/// no tenant to be staff *in*, so it is never previewable.
pub async fn may_preview(parts: &mut Parts) -> bool {
    let Some(ws) = parts.extensions.get::<entity::workspaces::Model>().cloned() else {
        return false;
    };
    let Some(org_id) = ws.org_id else {
        return false;
    };
    let Some(caller) = crate::Caller::from_extensions(&parts.extensions) else {
        return false;
    };
    let legacy = match oxy_platform::db::establish_connection().await {
        Ok(db) => crate::globals::platform_standing(&db, &caller)
            .await
            .is_staff(),
        // No database, no standing we can vouch for: previews fail closed.
        Err(_) => false,
    };
    authz::enforce_guard(
        parts,
        "guard.workspace_previewer",
        authz::Action::WorkspacePreview,
        authz::Resource::workspace(ws.id, org_id),
        legacy,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;

    fn make_parts_with<T: Clone + Send + Sync + 'static>(ext: T) -> Parts {
        let mut req = Request::builder().body(Body::empty()).unwrap();
        req.extensions_mut().insert(ext);
        req.into_parts().0
    }

    fn make_ctx(role: OrgRole) -> OrgContext {
        let now = chrono::Utc::now().fixed_offset();
        let org_id = uuid::Uuid::new_v4();
        OrgContext {
            org: entity::organizations::Model {
                id: org_id,
                name: "test".into(),
                slug: "test".into(),
                logo: None,
                logo_content_type: None,
                created_at: now,
                updated_at: now,
            },
            membership: entity::org_members::Model {
                id: uuid::Uuid::new_v4(),
                org_id,
                user_id: uuid::Uuid::new_v4(),
                role,
                created_at: now,
                updated_at: now,
            },
            // Tests construct a real membership; the synthetic-Owner fallback
            // path in `org_context.rs` is exercised separately. Keep `false`
            // unless a test explicitly wants to assert override semantics.
            is_global_override: false,
        }
    }

    fn make_workspace(created_by: Option<uuid::Uuid>) -> entity::workspaces::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::workspaces::Model {
            id: uuid::Uuid::new_v4(),
            name: "test-ws".into(),
            git_namespace_id: None,
            git_remote_url: None,
            created_at: now,
            updated_at: now,
            path: None,
            last_opened_at: None,
            created_by,
            org_id: None,
            status: entity::workspaces::WorkspaceStatus::Ready,
            error: None,
            monthly_vlm_budget_micros: None,
            current_revision_id: None,
            default_branch: None,
            repo_subdir: None,
        }
    }

    #[tokio::test]
    async fn org_owner_admits_owner() {
        let mut parts = make_parts_with(make_ctx(OrgRole::Owner));
        assert!(OrgOwner::from_request_parts(&mut parts, &()).await.is_ok());
    }

    #[tokio::test]
    async fn org_owner_rejects_admin() {
        let mut parts = make_parts_with(make_ctx(OrgRole::Admin));
        let err = OrgOwner::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn org_admin_admits_admin_and_owner() {
        let mut p1 = make_parts_with(make_ctx(OrgRole::Owner));
        assert!(OrgAdmin::from_request_parts(&mut p1, &()).await.is_ok());
        let mut p2 = make_parts_with(make_ctx(OrgRole::Admin));
        assert!(OrgAdmin::from_request_parts(&mut p2, &()).await.is_ok());
    }

    #[tokio::test]
    async fn org_admin_rejects_member() {
        let mut parts = make_parts_with(make_ctx(OrgRole::Member));
        let err = OrgAdmin::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn workspace_admin_rejects_member() {
        let mut parts = make_parts_with(EffectiveWorkspaceRole(WorkspaceRole::Member));
        let err = WorkspaceAdmin::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn workspace_admin_admits_admin() {
        let mut parts = make_parts_with(EffectiveWorkspaceRole(WorkspaceRole::Admin));
        assert!(
            WorkspaceAdmin::from_request_parts(&mut parts, &())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn workspace_editor_rejects_viewer() {
        let mut parts = make_parts_with(EffectiveWorkspaceRole(WorkspaceRole::Viewer));
        let err = WorkspaceEditor::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn workspace_editor_admits_member() {
        let mut parts = make_parts_with(EffectiveWorkspaceRole(WorkspaceRole::Member));
        assert!(
            WorkspaceEditor::from_request_parts(&mut parts, &())
                .await
                .is_ok()
        );
    }

    #[test]
    fn rename_helper_admits_org_owner_and_admin() {
        for role in [OrgRole::Owner, OrgRole::Admin] {
            let ctx = make_ctx(role);
            let ws = make_workspace(None);
            assert!(ensure_org_admin_or_workspace_creator(&ctx, &ws).is_ok());
        }
    }

    #[test]
    fn rename_helper_admits_member_when_creator() {
        let ctx = make_ctx(OrgRole::Member);
        let ws = make_workspace(Some(ctx.membership.user_id));
        assert!(ensure_org_admin_or_workspace_creator(&ctx, &ws).is_ok());
    }

    #[test]
    fn rename_helper_rejects_member_when_not_creator() {
        let ctx = make_ctx(OrgRole::Member);
        let ws = make_workspace(Some(uuid::Uuid::new_v4()));
        let err = ensure_org_admin_or_workspace_creator(&ctx, &ws).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn rename_helper_rejects_member_when_creator_unknown() {
        let ctx = make_ctx(OrgRole::Member);
        let ws = make_workspace(None);
        let err = ensure_org_admin_or_workspace_creator(&ctx, &ws).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }
}
