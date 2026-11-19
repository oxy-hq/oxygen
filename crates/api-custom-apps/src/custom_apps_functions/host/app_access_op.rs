//! `ctx.oxygen.*`: a platform action an org admin
//! approved for the app (`internal-docs/proposals/app-platform-access.md`).
//!
//! Three gates, in order, each failing closed:
//!
//! 1. the function's manifest entry asks for it (`FunctionCapabilities`, which
//!    the broker reads before this runs);
//! 2. the environment is production (the registry refuses the op anywhere
//!    else, `APP_ACCESS_FIX`);
//! 3. here: an org admin approved the action for this app (for an upload, on
//!    the pipeline the call names, which the function must also have listed),
//!    and `oxy-authz` allows it for the app's **service account**, whose only
//!    reach is that approval (`oxy_auth::token::app_access::credential_for`).
//!    The approval is read on every call, so a revoke is seen by the next one.
//!
//! An upload names a pipeline, never a workspace. Only after the approval is
//! the pipeline looked up among the org's compiled pipelines
//! (`AppPlatformActions::resolve_pipeline`), and access is decided on the
//! workspace that holds it.
//!
//! Then the platform does the work in this process
//! (`oxy_app_core::app_platform_seam`): no HTTP request, no network, no key.
//! A write records one audit row naming the app, its service account, the
//! function, the invocation and, on a route call, the person who asked.
//!
//! **Oxygen checks the app, not the person.** An approved app may let anyone it
//! serves ask for the action; deciding who may is the app's own code.

mod audit;
mod parse;

use entity::apps;
use entity::prelude::Apps;
use entity::users::UserStatus;
use oxy_app_core::app_platform_seam::{self, PlatformActionError};
use oxy_auth::AuthenticatedUser;
use oxy_auth::token::CredentialContext;
use oxy_auth::token::app_access;
use oxy_authz::{Action, Resource};
use oxy_server_authz::Caller;
use sea_orm::EntityTrait;
use serde_json::Value;
use uuid::Uuid;

pub(crate) use audit::trigger_of;
use parse::invalid;
pub(crate) use parse::{Request, parse};

use super::{CapabilityArea, HostError, HostOp, ProjectFunctionHost};

/// What a function without `oxygen.frontline.devices` sees: the broker's
/// refusal, in the host's own words.
pub const FRONTLINE_CAPABILITY_MISSING: &str = "OxygenCapabilityMissing: this function has not \
     declared the `oxygen.frontline.devices` capability (add \"oxygen\": { \"frontline\": { \"devices\": \
     \"manage\" } } to its oxy-app.json entry). An org admin must also approve it.";

/// What a function without `oxygen.sourceUploads` sees.
pub const SOURCE_UPLOADS_CAPABILITY_MISSING: &str = "OxygenCapabilityMissing: this function has not \
     declared the `oxygen.sourceUploads` capability (add \"oxygen\": { \"sourceUploads\": { \
     \"pipelines\": [\"<pipeline name>\"] } } to its oxy-app.json entry). An org admin must also \
     approve it.";

/// What a function sees when it names a pipeline its entry does not list.
pub(crate) fn pipeline_not_declared(pipeline: &str) -> HostError {
    HostError::capability_missing(
        CapabilityArea::Oxygen,
        &format!(
            "OxygenCapabilityMissing: this function has not declared the pipeline `{pipeline}` \
             (add it to \"oxygen\": {{ \"sourceUploads\": {{ \"pipelines\": [...] }} }} in its \
             oxy-app.json entry). An org admin must also approve it."
        ),
    )
}

/// The authz log label of the decision, per action.
fn label(action: Action) -> &'static str {
    match action {
        Action::FrontlineDeviceManage => "host.app_access.frontline_device_manage",
        _ => "host.app_access.source_upload_report",
    }
}

/// The approved action a host op performs. `None` for an op that is not one.
pub(crate) fn action_of(op: HostOp) -> Option<Action> {
    match op {
        HostOp::FrontlineDevicesList
        | HostOp::FrontlineDevicesCreate
        | HostOp::FrontlineDevicesReissueLink
        | HostOp::FrontlineDevicesRevoke => Some(Action::FrontlineDeviceManage),
        HostOp::SourceUploadsReport => Some(Action::SourceUploadReport),
        _ => None,
    }
}

/// Where an org admin approves an app, in the words every refusal uses.
const WHERE_TO_APPROVE: &str = "An owner or admin of the organization approves it under \
     Apps, on the organization's API tokens settings.";

/// The refusal for an app that holds no approval of `action`. A caller error:
/// an org admin's decision, which the app can only report.
fn not_approved(action: Action) -> HostError {
    let what = match action {
        Action::FrontlineDeviceManage => "manage the organization's kiosk devices",
        _ => "upload source reports",
    };
    HostError::backend(format!(
        "AppAccessNotApproved: an org admin has not approved this app to {what}. \
         {WHERE_TO_APPROVE}"
    ))
}

/// The refusal for an upload naming a pipeline the app is not approved for.
fn pipeline_not_approved(pipeline: &str) -> HostError {
    HostError::backend(format!(
        "AppAccessNotApproved: an org admin has not approved this app to upload reports for \
         the pipeline `{pipeline}`. {WHERE_TO_APPROVE}"
    ))
}

fn platform_error(e: PlatformActionError) -> HostError {
    match e {
        PlatformActionError::Invalid(m) => invalid(m),
        PlatformActionError::NotFound(m) => HostError::backend(format!("AppActionNotFound: {m}")),
        PlatformActionError::Unavailable(m) => {
            HostError::backend(format!("AppActionUnavailable: {m}"))
        }
    }
}

/// The resource an action is decided on: the org for kiosks, the workspace
/// that holds the named pipeline for an upload.
fn resource_of(org_id: Uuid, pipeline: Option<&app_platform_seam::ResolvedPipeline>) -> Resource {
    match pipeline {
        None => Resource::org(org_id),
        Some(found) => Resource::workspace(found.workspace_id, org_id),
    }
}

/// The shipped check beside the model (`existing_allow`), for the app's
/// account: what the route's guard asks of a member of the org. Kiosks are
/// the `OrgAdmin` ring, so an admin; an upload is `WorkspaceEditor`, so any
/// member. The model can only subtract from this.
fn shipped_check(action: Action, credential: &CredentialContext, org_id: Uuid) -> bool {
    credential.service_account.is_some_and(|account| {
        account.org_id == org_id && (account.admin || action == Action::SourceUploadReport)
    })
}

/// The person who asked, as the app's account sees them: nobody but the app.
fn account_user(credential: &CredentialContext) -> AuthenticatedUser {
    AuthenticatedUser {
        id: credential.principal_user_id,
        email: None,
        name: credential.name.clone(),
        picture: None,
        status: UserStatus::Active,
        credential: Some(credential.clone()),
    }
}

pub(super) async fn run(
    host: &ProjectFunctionHost,
    op: HostOp,
    payload: Value,
) -> Result<Value, HostError> {
    let action = action_of(op).ok_or_else(|| {
        HostError::internal(&format!("ctx.{} is not a platform action", op.name()))
    })?;
    let request = parse(op, &payload)?;
    let pipeline = declared_pipeline(host, &request)?;
    let (app, credential) = approved(host, action, pipeline.as_deref()).await?;
    let platform = app_platform_seam::installed().ok_or_else(|| {
        HostError::backend("AppAccessUnavailable: this process has no platform actions installed")
    })?;
    // Only an approved app learns where a pipeline is, or that it is not.
    let resolved = match &pipeline {
        Some(name) => Some(
            platform
                .resolve_pipeline(&host.db, app.org_id, name)
                .await
                .map_err(platform_error)?,
        ),
        None => None,
    };
    allowed(host, action, &app, &credential, resolved.as_ref()).await?;

    let result = match (request, resolved) {
        (Request::Frontline(device_op), _) => {
            platform
                .frontline_devices(
                    &host.db,
                    app.org_id,
                    device_op,
                    credential.principal_user_id,
                )
                .await
        }
        (Request::Upload(report), Some(found)) => {
            platform.source_upload_report(found, report).await
        }
        (Request::Upload(_), None) => {
            return Err(HostError::internal(
                "an upload reached the platform unresolved",
            ));
        }
    };
    audit::count_usage(host, op, &credential, &result).await;
    let reply = result.map_err(platform_error)?;
    audit::record(host, op, &app, &credential, &reply, pipeline.as_deref()).await;
    Ok(reply)
}

/// The pipeline an upload names, once the function's own list has it: a
/// pipeline its entry does not name is its manifest's mistake, whatever an
/// admin approved. `None` for a kiosk op.
fn declared_pipeline(
    host: &ProjectFunctionHost,
    request: &Request,
) -> Result<Option<String>, HostError> {
    match request {
        Request::Upload(report)
            if !host.caps.source_upload_pipelines.contains(&report.pipeline) =>
        {
            Err(pipeline_not_declared(&report.pipeline))
        }
        Request::Upload(report) => Ok(Some(report.pipeline.clone())),
        Request::Frontline(_) => Ok(None),
    }
}

/// The app and the credential it decides with, once an org admin has
/// approved `action` for it (for an upload, on `pipeline`). Read from the live
/// rows on every call, so a revoke is seen by the next one.
async fn approved(
    host: &ProjectFunctionHost,
    action: Action,
    pipeline: Option<&str>,
) -> Result<(apps::Model, CredentialContext), HostError> {
    let app = Apps::find_by_id(host.app_id)
        .one(&host.db)
        .await
        .map_err(|e| HostError::backend(format!("AppActionUnavailable: {e}")))?
        .filter(|app| app.org_id == host.org_id)
        .ok_or_else(|| not_approved(action))?;
    let read_failed = |e: oxy_shared::errors::OxyError| {
        tracing::error!(error = %e, app = %host.app_id, "app access could not be read");
        HostError::backend("AppActionUnavailable: the app's approval could not be read")
    };
    let credential = app_access::credential_for(&host.db, &app)
        .await
        .map_err(read_failed)?
        .filter(|c| {
            c.app_access
                .as_ref()
                .is_some_and(|reach| reach.grants.iter().any(|g| g.action == action))
        })
        .ok_or_else(|| not_approved(action))?;
    if let Some(pipeline) = pipeline {
        let grants = app_access::approvals(&host.db, app.id)
            .await
            .map_err(read_failed)?;
        if !app_access::approved_targets(&app, &grants, action).contains(pipeline) {
            return Err(pipeline_not_approved(pipeline));
        }
    }
    Ok((app, credential))
}

/// `oxy-authz`'s decision for the app's account, on the org for kiosks and on
/// the workspace holding the pipeline for an upload. Approved and still
/// refused means the account was demoted or disabled, or the app moved: the
/// org admin's to fix, as an approval is.
async fn allowed(
    host: &ProjectFunctionHost,
    action: Action,
    app: &apps::Model,
    credential: &CredentialContext,
    resolved: Option<&app_platform_seam::ResolvedPipeline>,
) -> Result<(), HostError> {
    let caller = Caller::of(&account_user(credential), Some(credential));
    let allowed = oxy_server_authz::enforce_for(
        &host.db,
        &caller,
        label(action),
        action,
        resource_of(app.org_id, resolved),
        shipped_check(action, credential, app.org_id),
    )
    .await;
    if allowed {
        Ok(())
    } else {
        Err(not_approved(action))
    }
}

#[cfg(test)]
#[path = "app_access_op_tests.rs"]
mod tests;
