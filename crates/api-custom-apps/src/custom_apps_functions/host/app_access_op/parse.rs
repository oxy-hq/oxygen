//! A platform action's arguments, read from the function's payload before
//! anything else. Every refusal here is the function's own argument, so it is
//! a caller error (`AppActionInvalid`) and never pages.

use base64::Engine as _;
use oxy_app_core::app_platform_seam::{FrontlineDevicesOp, SourceReport};
use oxy_custom_apps::platform_access::valid_pipeline_name;
use serde_json::Value;
use uuid::Uuid;

use super::super::{HostError, HostOp};

/// What the function asked for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Request {
    Frontline(FrontlineDevicesOp),
    Upload(SourceReport),
}

pub(super) fn invalid(message: impl std::fmt::Display) -> HostError {
    HostError::backend(format!("AppActionInvalid: {message}"))
}

fn text_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn uuid_field(payload: &Value, key: &str, op: &str) -> Result<Uuid, HostError> {
    let raw = text_field(payload, key)
        .ok_or_else(|| invalid(format!("{op} needs `{key}`, the device's id")))?;
    Uuid::parse_str(raw).map_err(|_| invalid(format!("`{key}` must be a device id (a UUID)")))
}

/// Read the payload of `op`.
pub(crate) fn parse(op: HostOp, payload: &Value) -> Result<Request, HostError> {
    let device = |op| Ok(Request::Frontline(op));
    match op {
        HostOp::FrontlineDevicesList => device(FrontlineDevicesOp::List),
        HostOp::FrontlineDevicesCreate => device(create(payload)?),
        HostOp::FrontlineDevicesReissueLink => device(FrontlineDevicesOp::ReissueLink {
            id: uuid_field(payload, "id", "ctx.oxygen.frontline.devices.reissueLink")?,
        }),
        HostOp::FrontlineDevicesRevoke => device(FrontlineDevicesOp::Revoke {
            id: uuid_field(payload, "id", "ctx.oxygen.frontline.devices.revoke")?,
        }),
        HostOp::SourceUploadsReport => Ok(Request::Upload(report(payload)?)),
        _ => Err(HostError::internal(&format!(
            "ctx.{} is not a platform action",
            op.name()
        ))),
    }
}

/// `ctx.oxygen.frontline.devices.create({ name, locationId?, returnTo?, idleTimeoutSeconds? })`.
fn create(payload: &Value) -> Result<FrontlineDevicesOp, HostError> {
    let name = text_field(payload, "name")
        .ok_or_else(|| invalid("ctx.oxygen.frontline.devices.create needs a `name`"))?
        .to_string();
    let location_id =
        match payload.get("locationId") {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) => Some(Uuid::parse_str(raw.trim()).map_err(|_| {
                invalid("`locationId` must be one of the org's location ids (a UUID)")
            })?),
            Some(_) => return Err(invalid("`locationId` must be a string")),
        };
    let idle_timeout_seconds = match payload.get("idleTimeoutSeconds") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid("`idleTimeoutSeconds` must be a whole number"))?,
        ),
    };
    Ok(FrontlineDevicesOp::Create {
        name,
        location_id,
        return_to: text_field(payload, "returnTo").map(str::to_string),
        idle_timeout_seconds,
    })
}

/// `ctx.oxygen.sourceUploads.report({ file, fileName, pipeline, workflowId, reportYear?, reportMonth? })`.
fn report(payload: &Value) -> Result<SourceReport, HostError> {
    let need = |key: &str| {
        text_field(payload, key)
            .map(str::to_string)
            .ok_or_else(|| invalid(format!("ctx.oxygen.sourceUploads.report needs `{key}`")))
    };
    let file = base64::engine::general_purpose::STANDARD
        .decode(need("file")?.as_bytes())
        .map_err(|_| invalid("`file` must be the report's bytes, base64-encoded"))?;
    let year = payload.get("reportYear").and_then(Value::as_i64);
    let month = payload
        .get("reportMonth")
        .and_then(Value::as_u64)
        .and_then(|m| u32::try_from(m).ok());
    let period = match (year, month) {
        (Some(y), Some(m)) => Some((y, m)),
        (None, None) => None,
        _ => {
            return Err(invalid(
                "`reportYear` and `reportMonth` must be given together",
            ));
        }
    };
    let pipeline = need("pipeline")?;
    if !valid_pipeline_name(&pipeline) {
        return Err(invalid(
            "`pipeline` must be a pipeline's name: 1–128 characters of [A-Za-z0-9_.-]",
        ));
    }
    Ok(SourceReport {
        file,
        file_name: need("fileName")?,
        pipeline,
        workflow_id: need("workflowId")?,
        period,
    })
}
