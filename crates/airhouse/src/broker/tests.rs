//! Subjects, role mapping, cache freshness and retry jitter.

use super::*;

#[test]
fn user_subject_renders_as_uuid() {
    let uid = Uuid::nil();
    assert_eq!(BrokerSubject::User(uid).audit_subject(), uid.to_string());
}

#[test]
fn system_subject_includes_workspace_and_purpose() {
    let ws = Uuid::nil();
    let s = BrokerSubject::System {
        workspace_id: ws,
        purpose: SystemPurpose::Scheduler,
    };
    assert_eq!(
        s.audit_subject(),
        format!("system:workspace:{ws}:scheduler")
    );
}

/// The previews' live-table read is its own audit segment, so a staff
/// check reading `information_schema` never reads as scheduler traffic.
#[test]
fn preview_subject_is_audited_as_preview() {
    let ws = Uuid::nil();
    let s = BrokerSubject::System {
        workspace_id: ws,
        purpose: SystemPurpose::Preview,
    };
    assert_eq!(s.audit_subject(), format!("system:workspace:{ws}:preview"));
    let ddl = BrokerSubject::System {
        workspace_id: ws,
        purpose: SystemPurpose::PreviewDdl,
    };
    assert_eq!(
        ddl.audit_subject(),
        format!("system:workspace:{ws}:preview-ddl")
    );
}

/// Production's app credential and staging's sibling credential are two
/// cache entries, though both audit as the app.
#[test]
fn an_app_mint_is_cached_per_schema() {
    let ws = Uuid::nil();
    let app = |schema: &str| BrokerSubject::App {
        workspace_id: ws,
        app_slug: "store-ops".into(),
        schema: schema.into(),
    };
    let production = cache_key(ws, &app("app_store_ops"), UserRole::Writer);
    let staging = cache_key(ws, &app("app_store_ops__staging"), UserRole::Writer);
    assert_ne!(production, staging);
    assert_eq!(
        production,
        cache_key(ws, &app("app_store_ops"), UserRole::Writer)
    );
    assert_ne!(
        cache_key(ws, &app("app_store_ops"), UserRole::Reader),
        cache_key(ws, &app("app_store_ops__staging"), UserRole::Reader),
        "an app's Reader entries are per schema too"
    );
    assert_eq!(
        app("app_store_ops").audit_subject(),
        app("app_store_ops__staging").audit_subject(),
        "both still audit as the app"
    );
    let user = BrokerSubject::User(Uuid::from_u128(1));
    assert_eq!(
        cache_key(ws, &user, UserRole::Reader).1,
        user.audit_subject(),
        "every other subject keys as before"
    );
}

#[test]
fn airhouse_role_mapping_matches_user_provisioner() {
    assert_eq!(airhouse_role_for(WorkspaceRole::Owner), UserRole::Admin);
    assert_eq!(airhouse_role_for(WorkspaceRole::Admin), UserRole::Writer);
    assert_eq!(airhouse_role_for(WorkspaceRole::Member), UserRole::Reader);
    assert_eq!(airhouse_role_for(WorkspaceRole::Viewer), UserRole::Reader);
}

#[test]
fn cache_entry_fresh_outside_buffer() {
    let cred = mock_cred(Utc::now() + chrono::Duration::seconds(3600));
    let entry = CacheEntry { cred };
    assert!(entry.is_fresh(Utc::now()));
}

#[test]
fn cache_entry_stale_inside_buffer() {
    let cred = mock_cred(Utc::now() + chrono::Duration::seconds(30));
    let entry = CacheEntry { cred };
    assert!(!entry.is_fresh(Utc::now()));
}

#[test]
fn cache_entry_stale_when_expired() {
    let cred = mock_cred(Utc::now() - chrono::Duration::seconds(1));
    let entry = CacheEntry { cred };
    assert!(!entry.is_fresh(Utc::now()));
}

#[test]
fn jittered_stays_within_envelope() {
    let base = Duration::from_millis(1000);
    for _ in 0..1000 {
        let j = jittered(base);
        assert!(j.as_millis() >= 800);
        assert!(j.as_millis() <= 1200);
    }
}

fn mock_cred(expires_at: DateTime<Utc>) -> EphemeralCredential {
    EphemeralCredential {
        username: "eph_test".into(),
        password: "tk_test".into(),
        tenant: "test".into(),
        role: "reader".into(),
        expires_at,
        service_account_id: "sa_test".into(),
        write_schemas: None,
    }
}
