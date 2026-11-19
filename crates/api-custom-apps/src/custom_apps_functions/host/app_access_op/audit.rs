//! What a platform action leaves behind: a count in the app row's daily usage
//! for every call, and one `audit_events` row for every write.

use chrono::Utc;
use entity::apps;
use oxy_app_core::app_platform_seam::PlatformActionError;
use oxy_app_core::audit;
use oxy_auth::token::CredentialContext;
use oxy_auth::token::usage::{self, UsageSample};
use oxy_authz::Action;
use serde_json::{Value, json};

use super::super::{HostOp, ProjectFunctionHost};

/// What set an invocation off, from its mode, in the words the audit row uses.
pub(crate) fn trigger_of(mode: &str) -> &'static str {
    match mode {
        "route" => "person",
        "schedule" => "schedule",
        "webhook" => "webhook",
        "event" => "event",
        "manual" => "manual",
        m if m.starts_with("airway") => "airway_step",
        _ => "unknown",
    }
}

/// The audit action a successful op records; `None` for a read.
fn audit_action(op: HostOp, reply: &Value) -> Option<&'static str> {
    match op {
        HostOp::FrontlineDevicesCreate => Some("frontline.device.created"),
        HostOp::FrontlineDevicesReissueLink => Some("frontline.device.link_reissued"),
        HostOp::FrontlineDevicesRevoke => {
            (reply.get("revoked") == Some(&Value::Bool(true))).then_some("frontline.device.revoked")
        }
        HostOp::SourceUploadsReport => Some("source_upload.report_uploaded"),
        _ => None,
    }
}

/// Count the call in the app row's daily usage, with the status the matching
/// route would have answered (a platform failure is not the caller's fault),
/// and set the row's last-used time.
pub(super) async fn count_usage(
    host: &ProjectFunctionHost,
    op: HostOp,
    credential: &CredentialContext,
    result: &Result<Value, PlatformActionError>,
) {
    let status = match result {
        Ok(_) => 200,
        Err(PlatformActionError::Invalid(_)) => 400,
        Err(PlatformActionError::NotFound(_)) => 404,
        Err(PlatformActionError::Unavailable(_)) => 502,
    };
    usage::record(UsageSample {
        token_id: credential.token_id,
        status,
        ip: None,
        user_agent: None,
        route: Some(format!("ctx.{}", op.name())),
        at: Utc::now(),
    });
    oxy_auth::token::store::touch_last_used(&host.db, credential).await;
}

/// One row for a write, as the app's account, naming the app, the function,
/// the invocation, what set it off and, for an upload, the pipeline. A read
/// records nothing.
pub(super) async fn record(
    host: &ProjectFunctionHost,
    op: HostOp,
    app: &apps::Model,
    credential: &CredentialContext,
    reply: &Value,
    pipeline: Option<&str>,
) {
    let Some(event) = audit_action(op, reply) else {
        return;
    };
    let identity = &host.identity;
    let target_id = reply
        .get("id")
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string())
        })
        .unwrap_or_default();
    let target_name = reply
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| reply.get("location").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    let target_type = match super::action_of(op) {
        Some(Action::FrontlineDeviceManage) => "frontline_device",
        _ => "source_upload",
    };
    let entry = oxy_custom_apps::write_record::app_action_entry(
        event,
        credential.principal_user_id,
        &identity.app_slug,
    )
    .org(app.org_id)
    .target(target_type, target_id, target_name)
    .metadata(json!({
        "token_id": credential.token_id,
        "token_name": credential.name,
        "token_kind": "app",
        "app_id": app.id,
        "app_slug": app.slug,
        "function": identity.function_name,
        "invocation_id": identity.invocation_id,
        "mode": identity.mode,
        // What set this call off, which the owner asked every row to say: a
        // person (with who), a schedule, a webhook, an event, a manual run or
        // an Airway step.
        "trigger": trigger_of(&identity.mode),
        "on_behalf_of": identity.user_id,
        "pipeline": pipeline,
    }));
    audit::record_best_effort(&host.db, entry).await;
}
