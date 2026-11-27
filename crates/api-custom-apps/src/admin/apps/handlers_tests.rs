//! The admin app handlers' own unit tests: publish-token scope, batch limits.

use super::*;

fn marker(app_id: Option<Uuid>) -> oxy_auth::types::AppPublishTokenAuth {
    oxy_auth::types::AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id,
        machine_identity: None,
    }
}

#[test]
fn a_live_build_published_via_trusted_publishing_is_ci() {
    // The OIDC path records its workflow identity and NO user — the machine
    // principal has no `users` row, so `published_by` stays NULL.
    let live = LivePublisher {
        by: None,
        via: Some(
            "github-oidc:acme/app/.github/workflows/oxy-publish.yml@refs/heads/main env=production"
                .into(),
        ),
    };
    assert_eq!(live_published_via(Some(&live)), Some("ci"));
}

#[test]
fn a_live_build_published_by_a_user_is_a_person() {
    // A session publish and a long-lived-token CI job both land here: the
    // token records the human who minted it.
    let live = LivePublisher {
        by: Some(Uuid::new_v4()),
        via: None,
    };
    assert_eq!(live_published_via(Some(&live)), Some("person"));
}

#[test]
fn nothing_live_or_no_publisher_says_nothing() {
    // No live build: a draft serves nobody, so there is no path to judge.
    assert_eq!(live_published_via(None), None);
    // A live build that predates both columns: unknown is not "person".
    assert_eq!(live_published_via(Some(&LivePublisher::default())), None);
}

#[test]
fn an_app_scoped_token_names_its_own_app_and_no_other() {
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    assert!(token_names_app(&marker(Some(mine)), mine));
    // The whole point of the route being on the allow-list: publishing to
    // an app already lets a token cause anything that app can cause, and
    // that argument covers exactly the app the token may publish to.
    assert!(!token_names_app(&marker(Some(mine)), theirs));
}

#[test]
fn an_unscoped_token_is_bounded_by_the_allow_list_alone() {
    // A staff token carries no app id; nothing here narrows it, and the
    // scope middleware is what keeps it to publish + checks.
    assert!(token_names_app(&marker(None), Uuid::new_v4()));
}

#[test]
fn validate_template_id_accepts_vite() {
    assert_eq!(validate_template_id(Some("vite")).unwrap(), "vite");
}

#[test]
fn validate_template_id_defaults_to_vite_when_none() {
    assert_eq!(validate_template_id(None).unwrap(), "vite");
}

#[test]
fn validate_template_id_rejects_unknown() {
    assert!(validate_template_id(Some("does-not-exist")).is_err());
}

#[test]
fn validate_display_name_accepts_typical_names() {
    assert!(validate_display_name("Acme Analytics").is_ok());
    assert!(validate_display_name("Store Pulse — v2 (beta)").is_ok());
    assert!(validate_display_name("数据看板").is_ok()); // unicode is fine
}

#[test]
fn validate_display_name_rejects_empty_or_whitespace_only() {
    assert!(validate_display_name("").is_err());
    assert!(validate_display_name("   ").is_err());
}

#[test]
fn validate_display_name_rejects_json_breaking_chars() {
    // Each of these would corrupt the rendered oxy-app.json /
    // package.json scaffolds when blindly substituted.
    assert!(validate_display_name("My \"App\"").is_err()); // double quote
    assert!(validate_display_name("path\\to").is_err()); // backslash
    assert!(validate_display_name("line\nbreak").is_err()); // newline
    assert!(validate_display_name("tab\there").is_err()); // tab
}

#[test]
fn validate_display_name_rejects_jsx_breaking_chars() {
    // The dashboard template's `<h1>{{APP_DISPLAY_NAME}}</h1>`
    // would break (or worse, render unintended markup) if these
    // landed there post-substitution.
    assert!(validate_display_name("<script>").is_err());
    assert!(validate_display_name("name>x").is_err());
    assert!(validate_display_name("name{x").is_err());
    assert!(validate_display_name("name}x").is_err());
}

#[test]
fn validate_display_name_rejects_overlong() {
    let s = "a".repeat(129);
    assert!(validate_display_name(&s).is_err());
    let s = "a".repeat(128);
    assert!(validate_display_name(&s).is_ok());
}

#[test]
fn validate_display_name_counts_chars_not_bytes() {
    // CJK names: 128 chars at 3 bytes each = 384 bytes. The old
    // byte-count check rejected at 43 chars (129 bytes).
    let cjk_128 = "数".repeat(128);
    assert!(validate_display_name(&cjk_128).is_ok());
    let cjk_129 = "数".repeat(129);
    assert!(validate_display_name(&cjk_129).is_err());
}

#[test]
fn is_valid_slug_accepts_lower_kebab() {
    assert!(is_valid_slug("acme"));
    assert!(is_valid_slug("acme-analytics"));
    assert!(is_valid_slug("a"));
    assert!(is_valid_slug("acme-2"));
    assert!(is_valid_slug(&"a".repeat(63)));
}

/// `events` would shadow the app's slug routes: `/customer-apps/{org}/events/…`
/// are the org's app events endpoints (`app_events::api`), so an app slugged
/// `events` would lose `/customer-apps/{org}/events/{health,logs,…}`.
#[test]
fn is_valid_slug_refuses_a_reserved_slug() {
    assert!(!is_valid_slug("events"));
    assert!(is_valid_slug("events-2"), "only the exact slug is reserved");
    assert!(is_valid_slug("app-events"));
}

#[test]
fn is_valid_slug_rejects_bad_shapes() {
    assert!(!is_valid_slug(""));
    assert!(!is_valid_slug("Acme")); // uppercase
    assert!(!is_valid_slug("acme--analytics")); // consecutive dashes
    assert!(!is_valid_slug("-acme")); // leading dash
    assert!(!is_valid_slug("acme-")); // trailing dash
    assert!(!is_valid_slug("acme.analytics")); // dot
    assert!(!is_valid_slug("acme/x")); // slash
    assert!(!is_valid_slug(&"a".repeat(64))); // too long
}

#[test]
fn batch_response_counts_ok_and_failed() {
    let resp = BatchResponse::from_results(vec![
        BatchItemResult::ok(Uuid::nil()),
        BatchItemResult::failed(Uuid::nil(), "App not found.".into()),
        BatchItemResult::ok(Uuid::nil()),
    ]);
    assert_eq!(resp.succeeded, 2);
    assert_eq!(resp.failed, 1);
    assert_eq!(resp.results.len(), 3);
}

#[test]
fn validate_batch_rejects_empty_and_oversized() {
    assert!(validate_batch(&[]).is_err());
    let too_many = vec![Uuid::nil(); MAX_BATCH_IDS + 1];
    assert!(validate_batch(&too_many).is_err());
    assert!(validate_batch(&[Uuid::nil()]).is_ok());
}
