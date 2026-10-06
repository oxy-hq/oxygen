use chrono::TimeZone;

use super::*;

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 6, hour, 0, 0).unwrap()
}

fn token(expires: u32, revoked: Option<u32>) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::from_u128(0x70),
        kind: "sandbox_agent".into(),
        principal_user_id: Uuid::from_u128(1),
        name: "agent".into(),
        display_prefix: "oxy_sbx_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: false,
        platform: true,
        partner: false,
        expires_at: Some(at(expires).fixed_offset()),
        last_used_at: None,
        created_at: at(1).fixed_offset(),
        created_by: None,
        revoked_at: revoked.map(|hour| at(hour).fixed_offset()),
        revoked_by: None,
        revoke_reason: None,
        source: "ui".into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn org(n: u128) -> Uuid {
    Uuid::from_u128(0xA000 + n)
}

fn app(n: u128) -> Uuid {
    Uuid::from_u128(0xB000 + n)
}

fn grant(kind: &str, org_n: u128) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: Uuid::from_u128(0x70),
        kind: kind.into(),
        org_id: org(org_n),
        workspace_id: None,
        role_ceiling: None,
        app_id: Some(app(org_n)),
        created_at: at(1).fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

/// A sandbox of org `org_n`'s app, named and run so that neither can be
/// mistaken for another org's in a serialised row.
fn queued(org_n: u128, handle: &str) -> QueuedSandbox {
    QueuedSandbox {
        org_id: org(org_n),
        app_id: app(org_n),
        environment: format!("dev-{handle}"),
        run_id: format!("sandbox-teardown-{handle}-run"),
    }
}

fn rows(
    token: &api_tokens::Model,
    grants: &[api_token_grants::Model],
    queued: &[QueuedSandbox],
) -> Vec<AuditEntry> {
    let context = AuditContext::default();
    let base = || system_entry(EXPIRED_SANDBOXES_QUEUED, &context);
    entries(token, at(9), grants, queued, base)
}

/// The whole row as it could be read: every field, metadata included.
fn serialised(row: &AuditEntry) -> String {
    format!("{row:?} {}", row.metadata)
}

/// One row per org of a granted app, and of a queued sandbox, once each: an
/// org whose app the token was granted gets its row even when none of its
/// sandboxes was left.
#[test]
fn the_event_goes_to_every_granted_apps_org_once() {
    let grants = [
        grant(api_token_grants::KIND_APP_SANDBOX, 2),
        grant(api_token_grants::KIND_APP_SANDBOX, 1),
        grant(api_token_grants::KIND_APP_SANDBOX, 2),
        grant("workspace", 9),
    ];
    let orgs = concerned_orgs(&grants, &[queued(2, "a"), queued(3, "b")]);
    assert_eq!(orgs, vec![org(1), org(2), org(3)]);
}

/// A token granted apps of two orgs, with sandboxes queued in both: each
/// org's row lists its own sandboxes, and the other org's app id, sandbox
/// name and run id appear nowhere in it. The rows are one event.
#[test]
fn a_row_names_only_the_sandboxes_of_its_own_org() {
    let grants = [
        grant(api_token_grants::KIND_APP_SANDBOX, 1),
        grant(api_token_grants::KIND_APP_SANDBOX, 2),
    ];
    let left = [queued(1, "alpha"), queued(2, "bravo"), queued(2, "charlie")];
    let rows = rows(&token(17, Some(9)), &grants, &left);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].metadata["event_id"], rows[1].metadata["event_id"],
        "one event, joined by its id"
    );

    for (row, own, other) in [(&rows[0], 1, 2), (&rows[1], 2, 1)] {
        assert_eq!(row.org_id, Some(org(own)));
        let listed = row.metadata["sandboxes"].as_array().expect("sandboxes");
        let expected: Vec<&QueuedSandbox> = left.iter().filter(|s| s.org_id == org(own)).collect();
        assert_eq!(listed.len(), expected.len(), "{}", row.metadata);
        for (shown, sandbox) in listed.iter().zip(&expected) {
            assert_eq!(
                shown,
                &json!({
                    "app_id": sandbox.app_id,
                    "environment": sandbox.environment,
                    "run_id": sandbox.run_id,
                })
            );
        }
        let text = serialised(row);
        for foreign in left.iter().filter(|s| s.org_id == org(other)) {
            for secret in [
                foreign.app_id.to_string(),
                foreign.org_id.to_string(),
                foreign.environment.clone(),
                foreign.run_id.clone(),
            ] {
                assert!(!text.contains(&secret), "org {own}'s row holds {secret}");
            }
        }
        // Nor how many the other org had: nothing in the row counts sandboxes.
        let keys: Vec<&String> = row.metadata.as_object().expect("metadata").keys().collect();
        assert!(
            !keys
                .iter()
                .any(|key| key.contains("count") || key.contains("total")),
            "{keys:?}"
        );
    }
}

/// An org whose app was granted and had no sandbox left still gets its row,
/// with an empty list and nothing of the org that had one.
#[test]
fn an_org_with_no_sandbox_left_gets_a_row_with_an_empty_list() {
    let grants = [
        grant(api_token_grants::KIND_APP_SANDBOX, 1),
        grant(api_token_grants::KIND_APP_SANDBOX, 2),
    ];
    let left = [queued(2, "bravo")];
    let rows = rows(&token(9, None), &grants, &left);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].org_id, Some(org(1)));
    assert_eq!(rows[0].metadata["sandboxes"], json!([]));
    let text = serialised(&rows[0]);
    for secret in [app(2).to_string(), org(2).to_string(), "bravo".to_string()] {
        assert!(!text.contains(&secret), "{secret}");
    }
    assert_eq!(
        rows[1].metadata["sandboxes"].as_array().map(Vec::len),
        Some(1)
    );
}

/// Every row says which of the two ended the token and when, and is the
/// system's; what it says of a sandbox is the app, the name and the run.
#[test]
fn the_detail_names_what_ended_the_token_and_each_sandbox() {
    let grants = [grant(api_token_grants::KIND_APP_SANDBOX, 2)];
    let rows = rows(&token(17, Some(9)), &grants, &[queued(2, "a")]);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.action, "token.expired_sandboxes_queued");
    assert_eq!(row.actor_email, "system");
    assert_eq!(row.target_id, Some(Uuid::from_u128(0x70).to_string()));
    let body = &row.metadata;
    assert_eq!(body["token_kind"], "sandbox_agent");
    assert_eq!(body["reason"], "token_ended");
    assert_eq!(body["ended_by"], "revoked");
    assert_eq!(body["ended_at"], at(9).to_rfc3339());
    assert_eq!(
        body["sandboxes"],
        json!([{ "app_id": app(2), "environment": "dev-a", "run_id": "sandbox-teardown-a-run" }])
    );
    assert_eq!(shared_detail(&token(9, None), at(9))["ended_by"], "expired");
    // Revoked after it had already expired: the expiry ended it.
    assert_eq!(
        shared_detail(&token(9, Some(17)), at(9))["ended_by"],
        "expired"
    );
    assert!(
        shared_detail(&token(9, None), at(9))
            .get("sandboxes")
            .is_none(),
        "the shared part of a row names no sandbox"
    );
}
