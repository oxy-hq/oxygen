use sea_orm::{ActiveValue, DatabaseBackend, QueryTrait};

use super::register::delete_device;
use super::*;

fn row() -> device_tokens::Model {
    let at = chrono::DateTime::parse_from_rfc3339("2020-01-02T09:00:00Z").unwrap();
    device_tokens::Model {
        id: Uuid::nil(),
        user_id: Uuid::nil(),
        platform: "apns".into(),
        token: "tok".into(),
        device_name: Some("iPhone".into()),
        created_at: at,
        last_seen_at: at,
        enabled: true,
        min_level: None,
        os_permission: None,
        os_disabled: None,
        app_version: None,
        web_push_p256dh: None,
        web_push_auth: None,
    }
}

fn patch(v: serde_json::Value) -> DevicePatch {
    serde_json::from_value(v).expect("a device patch")
}

fn refusal(v: serde_json::Value) -> DeviceError {
    merge(row(), patch(v)).expect_err("refused")
}

#[test]
fn a_device_serializes_every_field_and_its_platform_as_a_person_knows_it() {
    let v = serde_json::to_value(DeviceView::of(&row())).unwrap();
    assert_eq!(
        v,
        json!({ "id": Uuid::nil(), "platform": "ios", "name": "iPhone", "app_version": null,
                "created_at": "2020-01-02T09:00:00.000Z", "last_seen_at": "2020-01-02T09:00:00.000Z",
                "enabled": true, "min_level": null, "os_permission": null, "os_disabled": [] })
    );
    assert_eq!(platform_name("fcm"), "android");
    assert_eq!(platform_name("web"), "web");
}

#[test]
fn a_patch_sets_what_it_names_clears_nulls_and_bumps_last_seen() {
    let next = merge(
        row(),
        patch(
            json!({ "enabled": false, "min_level": "critical", "os_permission": "denied",
                      "os_disabled": ["oxygen.oxy.anomaly", "oxygen.oxy.anomaly", "oxygen"],
                      "app_version": " 2.1.0 ", "name": null }),
        ),
    )
    .unwrap();
    assert_eq!(next.enabled, ActiveValue::Set(false));
    assert_eq!(next.min_level, ActiveValue::Set(Some("critical".into())));
    assert_eq!(next.os_permission, ActiveValue::Set(Some("denied".into())));
    assert_eq!(
        next.os_disabled,
        ActiveValue::Set(Some(json!(["oxygen", "oxygen.oxy.anomaly"]))),
        "sorted, deduplicated"
    );
    assert_eq!(next.app_version, ActiveValue::Set(Some("2.1.0".into())));
    assert_eq!(next.device_name, ActiveValue::Set(None));
    assert!(matches!(next.last_seen_at, ActiveValue::Set(t) if t > row().last_seen_at));

    // An empty patch still says "I am here".
    let seen = merge(row(), patch(json!({}))).unwrap();
    assert_eq!(seen.enabled, ActiveValue::Unchanged(true));
    assert!(matches!(seen.last_seen_at, ActiveValue::Set(_)));
    let cleared = merge(row(), patch(json!({ "os_disabled": [] }))).unwrap();
    assert_eq!(cleared.os_disabled, ActiveValue::Set(None));
}

#[test]
fn a_patch_refuses_what_it_cannot_store() {
    let bad = DeviceError::BadRequest;
    assert_eq!(refusal(json!({ "min_level": "urgent" })), bad("bad_level"));
    assert_eq!(
        refusal(json!({ "os_permission": "maybe" })),
        bad("bad_os_permission")
    );
    assert_eq!(
        refusal(json!({ "os_disabled": [""] })),
        bad("bad_os_disabled")
    );
    assert_eq!(
        refusal(json!({ "os_disabled": ["x".repeat(201)] })),
        bad("bad_os_disabled")
    );
    assert_eq!(refusal(json!({ "name": "n".repeat(121) })), bad("bad_name"));
    assert_eq!(
        refusal(json!({ "app_version": "v".repeat(65) })),
        bad("bad_app_version")
    );
    assert!(
        serde_json::from_value::<DevicePatch>(json!({ "token": "steal" })).is_err(),
        "a token is not something a patch can move"
    );
}

/// The DELETE by token is scoped to the caller: the statement names both the
/// user and the token, so another user's registration of the same token
/// survives.
#[test]
fn unregistering_deletes_only_the_callers_row_for_the_token() {
    let me = Uuid::parse_str("11111111-1111-1111-1111-111111111111").expect("uuid");
    let sql = delete_device(me, "tok-a")
        .build(DatabaseBackend::Postgres)
        .to_string();
    assert_eq!(
        sql,
        r#"DELETE FROM "device_tokens" WHERE "device_tokens"."user_id" = '11111111-1111-1111-1111-111111111111' AND "device_tokens"."token" = 'tok-a'"#
    );
}
