use std::collections::{BTreeMap, BTreeSet};

use oxy::service::secret_manager::SecretInfo;
use uuid::Uuid;

use super::{
    MissingKey, SharedSecret, apply_shared, group_grants, missing_from, own_keys, shared_by_app,
    shared_names,
};
use crate::custom_apps_secrets::declared::{AppSecretEntry, DeclaredKey, EnvSource};

fn entry(key: &str, is_set: bool, required: bool) -> AppSecretEntry {
    AppSecretEntry {
        key: key.to_string(),
        is_set,
        declared: true,
        required,
        source: EnvSource::Manifest,
        description: None,
        secret_id: None,
        updated_at: None,
        updated_by_email: None,
        shared: false,
        inherits_production: false,
        inherits_staging: false,
        shared_from_workspace: None,
    }
}

fn row(id: u128, name: &str) -> SecretInfo {
    let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("a valid timestamp");
    SecretInfo {
        id: Uuid::from_u128(id),
        name: name.to_string(),
        description: None,
        created_at: at,
        updated_at: at,
        created_by: Uuid::from_u128(99),
        updated_by: None,
        is_active: true,
    }
}

fn declared(key: &str, required: bool) -> DeclaredKey {
    DeclaredKey {
        key: key.to_string(),
        required,
        description: None,
        source: EnvSource::Manifest,
        shared: false,
    }
}

fn names(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

const PULSE: u128 = 7;
const OPS: u128 = 8;

/// The Platform Pulse case in the app's own view: both keys declared, neither
/// stored for the app, both shared from the workspace — so neither is missing.
#[test]
fn a_shared_workspace_secret_is_not_missing_in_the_view() {
    let (slack, claude) = (Uuid::from_u128(1), Uuid::from_u128(2));
    let shared = BTreeMap::from([
        ("SLACK_BOT_TOKEN".to_string(), slack),
        ("ANTHROPIC_API_KEY".to_string(), claude),
    ]);
    let mut entries = vec![
        entry("ANTHROPIC_API_KEY", false, false),
        entry("GITHUB_TOKEN", false, true),
        entry("SLACK_BOT_TOKEN", false, false),
    ];
    assert!(apply_shared(&shared, &mut entries));
    assert_eq!(entries[0].shared_from_workspace, Some(claude));
    assert_eq!(entries[2].shared_from_workspace, Some(slack));
    assert!(entries[1].is_missing_required());
}

/// The app's own value wins at run time, so the view keeps saying "set" and
/// does not attribute it to the workspace.
#[test]
fn an_apps_own_value_is_not_marked_shared() {
    let shared = BTreeMap::from([("SLACK_BOT_TOKEN".to_string(), Uuid::from_u128(1))]);
    let mut entries = vec![entry("SLACK_BOT_TOKEN", true, true)];
    assert!(!apply_shared(&shared, &mut entries));
    assert_eq!(entries[0].shared_from_workspace, None);
}

/// The listing's version of the same case, from rows read once: the shared keys
/// and the app's own key are provided, only the rest is missing — required
/// first.
#[test]
fn missing_keys_leave_out_own_and_shared_ones() {
    let pulse = Uuid::from_u128(PULSE);
    let rows = vec![
        row(1, "SLACK_BOT_TOKEN"),
        row(2, "ANTHROPIC_API_KEY"),
        row(3, &format!("apps/{pulse}/GITHUB_TOKEN")),
    ];
    let shared = shared_by_app(&[(Uuid::from_u128(1), pulse)], &rows);
    let provided: BTreeSet<String> = own_keys(pulse, &rows)
        .into_iter()
        .chain(shared[&pulse].iter().cloned())
        .collect();
    let missing = missing_from(
        [
            declared("ANTHROPIC_API_KEY", false),
            declared("GITHUB_TOKEN", true),
            declared("SLACK_BOT_TOKEN", false),
            declared("DIGEST_ORG", false),
            declared("STRIPE_KEY", true),
        ],
        &provided,
    );
    let keys: Vec<(&str, bool)> = missing
        .iter()
        .map(|m| (m.key.as_str(), m.required))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("STRIPE_KEY", true),
            ("ANTHROPIC_API_KEY", false),
            ("DIGEST_ORG", false)
        ]
    );
    assert_eq!(
        missing[0],
        MissingKey {
            key: "STRIPE_KEY".into(),
            required: true,
            description: None
        }
    );
}

/// An app's own production keys only: not another app's, and not one of its
/// environments' `staging/KEY`.
#[test]
fn own_keys_are_the_apps_production_keys_alone() {
    let (pulse, ops) = (Uuid::from_u128(PULSE), Uuid::from_u128(OPS));
    let rows = vec![
        row(1, &format!("apps/{pulse}/GITHUB_TOKEN")),
        row(2, &format!("apps/{pulse}/staging/STAGING_ONLY")),
        row(3, &format!("apps/{ops}/OPS_TOKEN")),
        row(4, "WAREHOUSE_PASSWORD"),
    ];
    assert_eq!(own_keys(pulse, &rows), names(&["GITHUB_TOKEN"]));
}

/// A grant shares a secret only while it is an active, plain workspace secret
/// of the rows read: a deleted one (absent) and an app's own row never count.
#[test]
fn a_grant_shares_only_a_live_plain_workspace_secret() {
    let (pulse, ops) = (Uuid::from_u128(PULSE), Uuid::from_u128(OPS));
    let rows = vec![
        row(1, "SLACK_BOT_TOKEN"),
        row(2, &format!("apps/{ops}/OPS_TOKEN")),
    ];
    let grants = [
        (Uuid::from_u128(1), pulse),
        (Uuid::from_u128(1), ops),
        (Uuid::from_u128(2), pulse),
        (Uuid::from_u128(9), pulse),
    ];
    let by_app = shared_by_app(&grants, &rows);
    assert_eq!(by_app[&pulse], names(&["SLACK_BOT_TOKEN"]));
    assert_eq!(by_app[&ops], names(&["SLACK_BOT_TOKEN"]));

    let ids = BTreeSet::from([Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(9)]);
    assert_eq!(
        shared_names(&ids, &rows),
        BTreeMap::from([("SLACK_BOT_TOKEN".to_string(), Uuid::from_u128(1))])
    );
}

#[test]
fn grants_group_by_secret() {
    let (a, b) = (Uuid::from_u128(10), Uuid::from_u128(11));
    let (x, y) = (Uuid::from_u128(1), Uuid::from_u128(2));
    assert_eq!(
        group_grants(vec![(a, y), (b, x), (a, x)]),
        vec![
            SharedSecret {
                secret_id: a,
                app_ids: vec![x, y]
            },
            SharedSecret {
                secret_id: b,
                app_ids: vec![x]
            },
        ]
    );
}
