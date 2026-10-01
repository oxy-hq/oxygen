//! Tests for `declared`: parsing the `env` block, merging webhook keys, and
//! reconciling declarations against what is stored.

use super::*;
use serde_json::json;

fn app() -> Uuid {
    Uuid::nil()
}

fn stored(key: &str) -> StoredSecret {
    StoredSecret {
        key: key.to_string(),
        secret_id: Uuid::nil(),
        updated_at: chrono::Utc::now(),
        updated_by_email: Some("someone@oxy.tech".to_string()),
    }
}

#[test]
fn no_env_block_declares_nothing_and_is_not_an_error() {
    let (map, err) = declared_env(Some(&json!({ "slug": "x" })), app());
    assert!(map.is_empty());
    assert!(err.is_none(), "an absent block is not a misconfiguration");
}

#[test]
fn explicit_null_declares_nothing() {
    let (map, err) = declared_env(Some(&json!({ "env": null })), app());
    assert!(map.is_empty());
    assert!(err.is_none());
}

#[test]
fn parses_required_and_description() {
    let (map, err) = declared_env(
        Some(&json!({ "env": {
            "STRIPE_API_KEY": { "required": true, "description": "refund lookups" },
            "SLACK_WEBHOOK_URL": {},
        }})),
        app(),
    );
    assert!(err.is_none());
    assert!(map["STRIPE_API_KEY"].required);
    assert_eq!(
        map["STRIPE_API_KEY"].description.as_deref(),
        Some("refund lookups")
    );
    assert!(
        !map["SLACK_WEBHOOK_URL"].required,
        "required defaults to false — a declaration is documentation first"
    );
}

/// The lenience contract: a malformed block must not take the app's whole
/// secrets view down with it, but must not be silent either.
#[test]
fn malformed_env_degrades_to_nothing_but_reports_why() {
    let (map, err) = declared_env(Some(&json!({ "env": ["STRIPE_API_KEY"] })), app());
    assert!(map.is_empty());
    assert!(
        err.expect("a parse failure must be reported")
            .contains("env"),
        "the message has to name the block so it is actionable"
    );
}

#[test]
fn webhook_secret_vars_are_comma_split_for_rotation() {
    let manifests = vec![
        json!({ "webhook": { "secretVar": "UBER_KEY_A, UBER_KEY_B" } }),
        json!({ "webhook": { "secretVar": "TOAST_KEY" } }),
        json!({ "route": true }),
    ];
    let vars = webhook_secret_vars(manifests.iter());
    assert_eq!(
        vars,
        ["TOAST_KEY", "UBER_KEY_A", "UBER_KEY_B"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
}

#[test]
fn webhook_secret_vars_ignores_blanks() {
    let manifests = vec![json!({ "webhook": { "secretVar": "A,, ,B" } })];
    let vars = webhook_secret_vars(manifests.iter());
    assert_eq!(vars.len(), 2, "empty segments are not keys");
}

#[test]
fn a_webhook_key_is_required_by_construction() {
    let merged = merge_declared(BTreeMap::new(), ["SIG".to_string()].into_iter().collect());
    assert!(
        merged["SIG"].required,
        "an unset webhook secret 401s every delivery, so it is not optional"
    );
    assert_eq!(merged["SIG"].source, EnvSource::Webhook);
}

#[test]
fn an_explicit_declaration_wins_over_the_inferred_one() {
    let env = BTreeMap::from([(
        "SIG".to_string(),
        OxyAppEnvDecl {
            required: false,
            description: Some("authored".to_string()),
            shared: false,
        },
    )]);
    let merged = merge_declared(env, ["SIG".to_string()].into_iter().collect());
    assert_eq!(merged["SIG"].description.as_deref(), Some("authored"));
    assert_eq!(merged["SIG"].source, EnvSource::Manifest);
    assert!(!merged["SIG"].required, "the author's word is the one kept");
}

#[test]
fn reconcile_covers_all_three_states() {
    let declared = merge_declared(
        BTreeMap::from([
            (
                "SET_KEY".to_string(),
                OxyAppEnvDecl {
                    required: true,
                    description: None,
                    shared: false,
                },
            ),
            (
                "MISSING_KEY".to_string(),
                OxyAppEnvDecl {
                    required: true,
                    description: None,
                    shared: false,
                },
            ),
        ]),
        BTreeSet::new(),
    );
    let entries = reconcile(declared, vec![stored("SET_KEY"), stored("LEFTOVER")]);

    let by_key = |k: &str| entries.iter().find(|e| e.key == k).unwrap().clone();

    let set = by_key("SET_KEY");
    assert!(set.is_set && set.declared);

    let missing = by_key("MISSING_KEY");
    assert!(!missing.is_set && missing.declared);
    assert!(missing.is_missing_required());
    assert!(
        missing.secret_id.is_none(),
        "a key with nothing stored has no row to reveal or delete"
    );

    let leftover = by_key("LEFTOVER");
    assert!(leftover.is_set && !leftover.declared);
    assert_eq!(leftover.source, EnvSource::Undeclared);
    assert!(
        !leftover.required,
        "nothing asks for it, so it cannot be missing-required"
    );
}

#[test]
fn missing_required_sorts_first() {
    let declared = merge_declared(
        BTreeMap::from([
            (
                "AAA_SET".to_string(),
                OxyAppEnvDecl {
                    required: true,
                    description: None,
                    shared: false,
                },
            ),
            (
                "ZZZ_MISSING".to_string(),
                OxyAppEnvDecl {
                    required: true,
                    description: None,
                    shared: false,
                },
            ),
            (
                "MMM_OPTIONAL".to_string(),
                OxyAppEnvDecl {
                    required: false,
                    description: None,
                    shared: false,
                },
            ),
        ]),
        BTreeSet::new(),
    );
    let entries = reconcile(declared, vec![stored("AAA_SET")]);
    let order: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        order,
        vec!["ZZZ_MISSING", "MMM_OPTIONAL", "AAA_SET"],
        "action first, alphabetical second — a fresh deploy's question is \
         what still needs filling in"
    );
}

#[test]
fn an_app_with_no_declarations_still_lists_what_is_stored() {
    // The pre-existing world: keys written by `ctx.secrets.set`, nothing
    // declared anywhere. The view must not be empty.
    let entries = reconcile(BTreeMap::new(), vec![stored("QB_ACCESS_TOKEN")]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "QB_ACCESS_TOKEN");
    assert!(entries[0].is_set);
}
