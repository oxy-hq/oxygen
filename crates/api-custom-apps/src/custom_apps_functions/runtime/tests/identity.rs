//! `ctx.user` as the isolate sees it.

use super::*;

fn full_identity() -> CtxUser {
    CtxUser {
        id: "11111111-1111-1111-1111-111111111111".into(),
        email: Some("ada@acme.com".into()),
        org_id: "22222222-2222-2222-2222-222222222222".into(),
        name: Some("Ada Lovelace".into()),
        picture: Some("https://cdn.example/ada.png".into()),
        app_role: Some("admin".into()),
        org_role: Some("member".into()),
        teams: vec![CtxTeam {
            id: "33333333-3333-3333-3333-333333333333".into(),
            name: "Finance".into(),
        }],
        kind: CtxIdentityKind::User,
        reach: oxy_app_core::operating_graph::reach::Reach::everywhere("app-admin", Vec::new()),
    }
}

/// The wire contract the SDK's `OxyFunctionUser` is typed against. Every key
/// here is camelCase — `org_id` shipped snake-cased once and made the
/// documented `ctx.user.orgId` read `undefined`, so the casing is pinned.
#[test]
fn ctx_user_serializes_the_documented_camel_case_keys() {
    let json: serde_json::Value = serde_json::to_value(full_identity()).unwrap();
    assert_eq!(json["orgId"], "22222222-2222-2222-2222-222222222222");
    assert_eq!(json["appRole"], "admin");
    assert_eq!(json["orgRole"], "member");
    assert_eq!(json["name"], "Ada Lovelace");
    assert_eq!(json["picture"], "https://cdn.example/ada.png");
    assert_eq!(json["kind"], "user");
    assert_eq!(json["teams"][0]["name"], "Finance");
    assert!(
        json.get("org_id").is_none(),
        "the host serializes orgId; the snake alias is added in __buildCtx, not here"
    );
}

#[test]
fn a_frontline_user_serializes_email_as_null_not_absent_and_not_empty() {
    // The contract app code branches on. `null` is load-bearing three ways:
    // `""` would look like an address to `ctx.email.send` and to any string
    // concat; an ABSENT key reads as `undefined`, which a `=== null` check
    // misses; and only an explicit null lets a function say "this person
    // cannot be emailed" without guessing.
    let json: serde_json::Value = serde_json::to_value(CtxUser {
        id: "11111111-1111-1111-1111-111111111111".into(),
        email: None,
        org_id: "22222222-2222-2222-2222-222222222222".into(),
        name: Some("Maria S.".into()),
        picture: None,
        app_role: None,
        org_role: None,
        teams: Vec::new(),
        kind: CtxIdentityKind::User,
        reach: oxy_app_core::operating_graph::reach::Reach::nowhere(),
    })
    .unwrap();
    assert_eq!(
        json.get("email"),
        Some(&serde_json::Value::Null),
        "email must be present and null, never omitted or empty: {json}"
    );
}

/// A schedule tick runs under the org owner's `user_id` but has no human
/// behind it. Absent human fields are what lets a function tell the two
/// apart without sniffing the synthetic email.
#[test]
fn system_identity_omits_every_human_field() {
    let json: serde_json::Value = serde_json::to_value(CtxUser {
        email: Some("schedule+rollup@system.oxy".into()),
        name: None,
        picture: None,
        org_role: None,
        teams: Vec::new(),
        kind: CtxIdentityKind::System,
        ..full_identity()
    })
    .unwrap();
    assert_eq!(json["kind"], "system");
    for absent in ["name", "picture", "orgRole"] {
        assert!(json.get(absent).is_none(), "{absent} must be absent");
    }
    assert_eq!(json["teams"], serde_json::json!([]));
}

/// The isolate's view, end to end: `ctx.user.orgId` is what the SDK types
/// promise, and the legacy `ctx.user.org_id` still resolves so functions
/// written against the shipped snake key keep working.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolate_sees_camel_case_identity_and_the_legacy_org_id_alias() {
    let (result, _host) = run_with_mock_ctx(
        r#"
            export default async (req, ctx) => Response.json({
                orgId: ctx.user.orgId,
                legacy: ctx.user.org_id,
                name: ctx.user.name,
                orgRole: ctx.user.orgRole,
                team: ctx.user.teams[0].name,
                kind: ctx.user.kind,
            });
        "#,
        InvocationCtx {
            user: full_identity(),
            env: Default::default(),
            airhouse_schema: None,
            channel: "production".into(),
        },
    )
    .await;

    let body = result.expect("function must resolve").body;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["orgId"], "22222222-2222-2222-2222-222222222222");
    assert_eq!(
        parsed["legacy"], parsed["orgId"],
        "the back-compat mirror must track orgId, not drift from it"
    );
    assert_eq!(parsed["name"], "Ada Lovelace");
    assert_eq!(parsed["orgRole"], "member");
    assert_eq!(parsed["team"], "Finance");
    assert_eq!(parsed["kind"], "user");
}
