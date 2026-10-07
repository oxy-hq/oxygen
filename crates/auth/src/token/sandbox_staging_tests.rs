//! The `staging` option of a sandbox agent mint, with no database: what a
//! body may say, what a stored mint keeps, and the grant row it becomes.

use chrono::Utc;
use sea_orm::ActiveValue::Set;
use serde_json::json;

use super::*;

const APP_A: &str = "0199f3a0-0000-7000-8000-00000000000a";

fn body(extra: serde_json::Value) -> Value {
    let mut body = json!({ "name": "agent", "kind": "sandbox_agent", "apps": [APP_A] });
    if let (Value::Object(body), Value::Object(extra)) = (&mut body, extra) {
        body.extend(extra);
    }
    body
}

#[test]
fn staging_defaults_to_false() {
    assert!(!parse(&body(json!({})), None).unwrap().staging);
    assert!(
        !parse(&body(json!({ "staging": null })), None)
            .unwrap()
            .staging
    );
    assert!(
        !parse(&body(json!({ "staging": false })), None)
            .unwrap()
            .staging
    );
}

#[test]
fn staging_true_is_read() {
    let mint = parse(&body(json!({ "staging": true })), None).unwrap();
    assert!(mint.staging);
    // Nothing else about the request moves with it.
    let plain = parse(&body(json!({})), None).unwrap();
    assert_eq!(
        MintRequest {
            staging: false,
            ..mint
        },
        plain
    );
}

#[test]
fn anything_but_a_boolean_is_refused() {
    for not_a_boolean in [
        json!("true"),
        json!("yes"),
        json!(1),
        json!(0),
        json!(["staging"]),
        json!({ "apps": [APP_A] }),
    ] {
        let refused = parse(&body(json!({ "staging": not_a_boolean })), None)
            .unwrap_err()
            .0;
        assert!(
            refused.starts_with("'staging' must be true or false"),
            "{not_a_boolean}: {refused}"
        );
    }
}

#[test]
fn a_stored_mint_keeps_staging_and_reads_back_the_same() {
    let app = Uuid::parse_str(APP_A).unwrap();
    let mint = parse(&body(json!({ "staging": true })), None).unwrap();
    let stored = mint.stored(&[app]);
    assert_eq!(stored["staging"], json!(true));
    assert_eq!(parse(&stored, None), Ok(mint));
}

/// A mint without the option is stored as it was before the option existed:
/// no `staging` key at all, so the two releases store the same bytes for it.
#[test]
fn a_stored_mint_without_staging_has_no_such_key() {
    let app = Uuid::parse_str(APP_A).unwrap();
    for asked in [json!({}), json!({ "staging": false })] {
        let mint = parse(&body(asked), None).unwrap();
        let stored = mint.stored(&[app]);
        assert!(stored.get("staging").is_none(), "{stored}");
        assert_eq!(
            stored,
            json!({
                "kind": "sandbox_agent",
                "name": "agent",
                "apps": [APP_A],
                "expires_in_hours": DEFAULT_HOURS,
            })
        );
        assert_eq!(parse(&stored, None), Ok(mint));
    }
}

/// The row beside an app's `app_sandbox` grant: the same org and app, the
/// kind alone differing, and no workspace or ceiling for admission to read.
#[test]
fn the_staging_grant_is_the_app_grant_under_another_kind() {
    let (org, app, token) = (Uuid::from_u128(7), Uuid::from_u128(9), Uuid::from_u128(1));
    let now = Utc::now().fixed_offset();
    let sandbox = GrantRow::app_sandbox(org, app).for_token(token, now);
    let staging = GrantRow::app_staging(org, app).for_token(token, now);
    assert_eq!(staging.kind, Set("app_staging".to_string()));
    assert_eq!(sandbox.kind, Set("app_sandbox".to_string()));
    assert_eq!(staging.org_id, sandbox.org_id);
    assert_eq!(staging.app_id, Set(Some(app)));
    assert_eq!(staging.workspace_id, Set(None));
    assert_eq!(staging.role_ceiling, Set(None));
    assert_eq!(staging.revoked_at, Set(None));
    assert_eq!(staging.created_at, Set(now));
}
