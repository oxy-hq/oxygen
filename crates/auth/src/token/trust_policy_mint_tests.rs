use serde_json::json;
use uuid::Uuid;

use super::*;

fn app() -> Uuid {
    Uuid::from_u128(7)
}

fn approval(extra: Value) -> Value {
    let mut mint = json!({
        "kind": KIND,
        "org": "acme",
        "account": "deployer",
        "app": app().to_string(),
        "repository": "acme/store",
        "workflow_path": ".github/workflows/oxy-publish.yml",
        "environment": "oxy-publish",
    });
    if let (Value::Object(mint), Value::Object(extra)) = (&mut mint, extra) {
        mint.extend(extra);
    }
    mint
}

fn refused(body: Value) -> String {
    parse(&body).expect_err("refused").0
}

#[test]
fn an_approval_names_the_account_the_app_and_the_run() {
    let parsed = parse(&approval(json!({ "create_account": true }))).unwrap();
    assert_eq!(
        parsed,
        PolicyRequest {
            org: "acme".into(),
            account: "deployer".into(),
            create_account: true,
            app: app().to_string(),
            repository: "acme/store".into(),
            workflow_path: ".github/workflows/oxy-publish.yml".into(),
            environment: Some("oxy-publish".into()),
        }
    );
    // Not asked to create: the account must exist.
    assert!(!parse(&approval(json!({}))).unwrap().create_account);
}

#[test]
fn the_policy_grants_publishing_the_app_and_nothing_else() {
    let body = parse(&approval(json!({}))).unwrap().policy_body(app());
    let grants = body.grants.expect("grants");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].kind.as_deref(), Some("app_publish"));
    assert_eq!(grants[0].app_id, Some(app()));
    assert_eq!(body.ref_pattern, None);
    assert_eq!(body.allow_self_hosted, None);
    // A grant cannot be asked for.
    assert!(refused(approval(json!({ "grants": [] }))).contains("publishing the app"));
}

#[test]
fn a_token_s_fields_are_refused_by_name() {
    for field in ["role", "name", "expires_in_days"] {
        let mut extra = Map::new();
        extra.insert(field.to_string(), json!("x"));
        let why = refused(approval(Value::Object(extra)));
        assert!(why.contains("mints no token"), "{field}: {why}");
    }
    assert!(refused(approval(json!({ "apps": [] }))).contains("not a field"));
}

#[test]
fn the_run_is_held_to_the_trust_policy_rules() {
    assert!(refused(approval(json!({ "repository": "not a repo" }))).contains("repository"));
    assert!(refused(approval(json!({ "workflow_path": "release.yml" }))).contains("workflow"));
    assert!(refused(approval(json!({ "account": "Deployer" }))).starts_with("'account'"));
    // An app by slug is read as asked; the approval resolves it in the org.
    assert_eq!(
        parse(&approval(json!({ "app": "store" }))).unwrap().app,
        "store"
    );
    assert!(refused(approval(json!({ "app": "" }))).contains("'app'"));
    assert!(refused(approval(json!({ "app": 7 }))).contains("'app'"));
    assert!(refused(approval(json!({ "create_account": "yes" }))).contains("true or false"));
}

/// A run is matched on the repository's ids and the approver sees only its
/// name: ids from the asker would be a trust anchor nobody checked.
#[test]
fn the_repository_s_ids_are_never_taken_from_the_asker() {
    for field in ["repository_id", "repository_owner_id"] {
        let mut extra = Map::new();
        extra.insert(field.to_string(), json!(987));
        let why = refused(approval(Value::Object(extra)));
        assert!(why.contains("from GitHub"), "{field}: {why}");
    }
    let body = parse(&approval(json!({}))).unwrap().policy_body(app());
    assert_eq!(body.repository_id, None);
    assert_eq!(body.repository_owner_id, None);
}

#[test]
fn only_its_own_kind_is_read() {
    assert!(asked_for(&approval(json!({}))));
    assert!(!asked_for(&json!({ "kind": "service_account" })));
    assert!(refused(json!({ "kind": "agent" })).contains("'kind'"));
}
