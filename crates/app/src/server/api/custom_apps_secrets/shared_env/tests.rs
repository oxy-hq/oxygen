use serde_json::json;

use super::*;

fn keys(set: BTreeSet<String>) -> Vec<String> {
    set.into_iter().collect()
}

#[test]
fn shared_is_read_per_key_and_defaults_to_false() {
    let manifest = json!({ "env": {
        "POKEHOUSE_API_KEY": { "shared": true },
        "QB_REFRESH_TOKEN": { "required": true },
    }});
    assert_eq!(
        keys(shared_env_keys(Some(&manifest))),
        ["POKEHOUSE_API_KEY"]
    );
    assert!(shared_env_keys(Some(&json!({ "env": "nonsense" }))).is_empty());
    assert!(shared_env_keys(None).is_empty());
}

/// The review's escape: a staging bundle marks production's written token and
/// webhook key `shared`. Production's build does not, so nothing is shared.
#[test]
fn a_staging_build_alone_shares_nothing_production_does_not() {
    let staging = json!({ "env": {
        "QB_REFRESH_TOKEN": { "shared": true },
        "UBER_SIGNING": { "shared": true },
        "POKEHOUSE_API_KEY": { "shared": true },
    }});
    let production = json!({ "env": { "POKEHOUSE_API_KEY": { "shared": true } } });
    assert_eq!(
        keys(effective_shared(Some(&staging), Some(&production))),
        ["POKEHOUSE_API_KEY"]
    );
    assert!(
        effective_shared(Some(&staging), None).is_empty(),
        "an app never promoted shares nothing"
    );
    assert!(
        effective_shared(None, Some(&production)).is_empty(),
        "production marking a key does not share it for a staging build that does not"
    );
}

#[test]
fn a_webhook_secret_of_either_build_is_never_shared() {
    let both = json!({ "env": { "SIG": { "shared": true }, "READ": { "shared": true } } });
    let mut production = both.clone();
    production["functions"] = json!({ "hook": { "webhook": { "secretVar": "OLD, SIG" } } });
    assert_eq!(
        keys(effective_shared(Some(&both), Some(&production))),
        ["READ"]
    );
    let mut staging = both.clone();
    staging["functions"] = json!({ "hook": { "webhook": { "secretVar": "SIG" } } });
    assert_eq!(
        keys(effective_shared(Some(&staging), Some(&both))),
        ["READ"]
    );
}

fn specs() -> Vec<(String, serde_json::Value)> {
    vec![
        (
            "refresh".to_string(),
            json!({ "secrets": { "write": true } }),
        ),
        (
            "hook".to_string(),
            json!({ "webhook": { "secretVar": "SIG" } }),
        ),
    ]
}

#[test]
fn publish_reads_the_bundled_function_and_its_webhook_block() {
    let manifest = json!({ "env": {
        "TOKEN": { "shared": true },
        "READ_ONLY": { "shared": true },
        "SIG": { "shared": true },
    }});
    let files = vec![(
        "functions/refresh.js".to_string(),
        br#"async(r,c)=>c.secrets.set("TOKEN",x)"#.to_vec(),
    )];
    let conflicts = check_shared_env(Some(&manifest), &specs(), &files, None).unwrap_err();
    assert_eq!(conflicts.len(), 2, "{conflicts:?}");
    let read_only = json!({ "env": { "READ_ONLY": { "shared": true } } });
    assert!(check_shared_env(Some(&read_only), &specs(), &files, None).is_ok());
    assert!(check_shared_env(None, &specs(), &files, None).is_ok());
}

/// A key the build being published neither writes nor verifies with, but the
/// build production serves does, is refused all the same.
#[test]
fn publish_checks_the_build_production_serves_too() {
    let manifest = json!({ "env": {
        "QB_REFRESH_TOKEN": { "shared": true },
        "UBER_SIGNING": { "shared": true },
        "READ_ONLY": { "shared": true },
    }});
    let production = BuildFunctions::production(
        "prod-7",
        vec![
            (
                "refresh".to_string(),
                json!({ "secrets": { "write": true } }),
                r#"const{secrets:s}=c;await s.set("QB_REFRESH_TOKEN",t)"#.to_string(),
            ),
            (
                "uber".to_string(),
                json!({ "webhook": { "secretVar": "UBER_SIGNING" } }),
                String::new(),
            ),
        ],
    );
    let conflicts = check_shared_env(Some(&manifest), &[], &[], Some(&production)).unwrap_err();
    assert_eq!(conflicts.len(), 2, "{conflicts:?}");
    assert!(
        conflicts
            .iter()
            .all(|c| c.contains("production's build `prod-7`")),
        "{conflicts:?}"
    );
    assert!(check_shared_env(Some(&manifest), &[], &[], None).is_ok());
}

#[test]
fn manifest_functions_lists_the_declared_functions() {
    let manifest = json!({ "functions": { "a": { "route": true }, "b": {} } });
    let names: Vec<String> = manifest_functions(Some(&manifest))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["a", "b"]);
    assert!(manifest_functions(None).is_empty());
}
