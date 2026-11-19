//! The part of an app's platform action decided before anything is read:
//! which action an op is, and what its arguments say. The approval, the
//! decision and the work are driven end to end by the integration group.

use oxy_app_core::app_platform_seam::{FrontlineDevicesOp, SourceReport};
use oxy_authz::Action;
use serde_json::json;
use uuid::Uuid;

use super::{
    Request, action_of, not_approved, parse, pipeline_not_approved, pipeline_not_declared,
};
use crate::custom_apps_functions::env_policy::HostOp;

#[test]
fn every_platform_op_is_one_approved_action_and_no_other_op_is() {
    for op in HostOp::ALL {
        let expected = match op {
            HostOp::FrontlineDevicesList
            | HostOp::FrontlineDevicesCreate
            | HostOp::FrontlineDevicesReissueLink
            | HostOp::FrontlineDevicesRevoke => Some(Action::FrontlineDeviceManage),
            HostOp::SourceUploadsReport => Some(Action::SourceUploadReport),
            _ => None,
        };
        assert_eq!(action_of(*op), expected, "{op:?}");
    }
    // Every app action has an op that performs it.
    for action in Action::APP_ACTIONS {
        assert!(
            HostOp::ALL.iter().any(|op| action_of(*op) == Some(action)),
            "{action:?} has no host op"
        );
    }
}

#[test]
fn a_create_reads_its_fields() {
    let location = Uuid::from_u128(7);
    let got = parse(
        HostOp::FrontlineDevicesCreate,
        &json!({
            "name": " Front counter ",
            "locationId": location.to_string(),
            "returnTo": "https://store-ops.example/desk",
            "idleTimeoutSeconds": 900
        }),
    )
    .unwrap();
    assert_eq!(
        got,
        Request::Frontline(FrontlineDevicesOp::Create {
            name: "Front counter".into(),
            location_id: Some(location),
            return_to: Some("https://store-ops.example/desk".into()),
            idle_timeout_seconds: Some(900),
        })
    );
}

#[test]
fn bad_arguments_are_the_functions_own_error() {
    let cases = [
        (HostOp::FrontlineDevicesCreate, json!({})),
        (HostOp::FrontlineDevicesCreate, json!({ "name": "  " })),
        (
            HostOp::FrontlineDevicesCreate,
            json!({ "name": "x", "locationId": "not-a-uuid" }),
        ),
        (
            HostOp::FrontlineDevicesCreate,
            json!({ "name": "x", "idleTimeoutSeconds": -1 }),
        ),
        (HostOp::FrontlineDevicesRevoke, json!({})),
        (HostOp::FrontlineDevicesReissueLink, json!({ "id": "nope" })),
        (HostOp::SourceUploadsReport, json!({ "file": "@@@" })),
        (
            HostOp::SourceUploadsReport,
            json!({
                "file": "YQ==", "fileName": "r.csv", "pipeline": "p", "workflowId": "w",
                "reportYear": 2026
            }),
        ),
        // A pipeline is named, never addressed by path.
        (
            HostOp::SourceUploadsReport,
            json!({
                "file": "YQ==", "fileName": "r.csv", "workflowId": "w",
                "pipeline": "pipelines/ubereats_transactions.airway.yml"
            }),
        ),
        (
            HostOp::SourceUploadsReport,
            json!({ "file": "YQ==", "fileName": "r.csv", "workflowId": "w" }),
        ),
    ];
    for (op, payload) in cases {
        let err = parse(op, &payload).expect_err(&format!("{op:?} {payload}"));
        assert!(
            err.message.starts_with("AppActionInvalid: "),
            "{op:?} {payload}: {}",
            err.message
        );
    }
}

#[test]
fn a_report_is_decoded_and_its_period_kept_whole() {
    let got = parse(
        HostOp::SourceUploadsReport,
        &json!({
            "file": "aGVsbG8=",
            "fileName": "report.csv",
            "pipeline": "ubereats_transactions",
            "workflowId": "wf-1",
            "reportYear": 2026,
            "reportMonth": 10
        }),
    )
    .unwrap();
    assert_eq!(
        got,
        Request::Upload(SourceReport {
            file: b"hello".to_vec(),
            file_name: "report.csv".into(),
            pipeline: "ubereats_transactions".into(),
            workflow_id: "wf-1".into(),
            period: Some((2026, 10)),
        })
    );
}

#[test]
fn a_list_takes_nothing_and_names_no_org() {
    // Whatever the function passes, the org is the host's.
    assert_eq!(
        parse(
            HostOp::FrontlineDevicesList,
            &json!({ "orgId": Uuid::nil().to_string() })
        )
        .unwrap(),
        Request::Frontline(FrontlineDevicesOp::List)
    );
}

#[test]
fn the_refusals_name_the_pipeline_and_where_to_approve() {
    let undeclared = pipeline_not_declared("toast_pos");
    assert!(
        undeclared.message.starts_with(
            "OxygenCapabilityMissing: this function has not declared the pipeline `toast_pos`"
        ),
        "{}",
        undeclared.message
    );
    let unapproved = pipeline_not_approved("toast_pos");
    assert!(
        unapproved.message.starts_with("AppAccessNotApproved: "),
        "{}",
        unapproved.message
    );
    assert!(unapproved.message.contains("`toast_pos`"));
    for message in [
        unapproved.message,
        not_approved(Action::FrontlineDeviceManage).message,
    ] {
        assert!(message.contains("under Apps"), "{message}");
        assert!(!message.contains("workspace"), "{message}");
    }
}
