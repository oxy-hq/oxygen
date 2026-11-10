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

/// A person's read on the shared, role-keyed query pool is audited as the
/// pool, never as a user: one connection serves every requester at that role,
/// so naming one of them would be a false row. Coarse and true over precise
/// and false — the individual is recorded by oxy's own audit trail.
#[test]
fn shared_query_subject_names_the_pool_not_a_user() {
    let ws = Uuid::nil();
    let s = BrokerSubject::System {
        workspace_id: ws,
        purpose: SystemPurpose::SharedQuery,
    };
    assert_eq!(
        s.audit_subject(),
        format!("system:workspace:{ws}:shared-query")
    );
    // Distinct from the background segment, so a person's query is never
    // filtered away as scheduler or agentic traffic.
    let bg = BrokerSubject::System {
        workspace_id: ws,
        purpose: SystemPurpose::AgenticBackground,
    };
    assert_ne!(s.audit_subject(), bg.audit_subject());
    // And it never renders as a user id.
    assert_ne!(s.audit_subject(), BrokerSubject::User(ws).audit_subject());
}

/// Only the shared query pool tells Airhouse its connection is shared: every
/// other subject is one requester's, and a claim there would refuse settings
/// that requester may legitimately use.
#[test]
fn only_the_shared_query_pool_asks_for_a_shared_session() {
    let ws = Uuid::nil();
    let shared = BrokerSubject::System {
        workspace_id: ws,
        purpose: SystemPurpose::SharedQuery,
    };
    assert!(shared.shared_session());
    for purpose in [
        SystemPurpose::Scheduler,
        SystemPurpose::AgenticBackground,
        SystemPurpose::SchemaCrawler,
    ] {
        let s = BrokerSubject::System {
            workspace_id: ws,
            purpose,
        };
        assert!(!s.shared_session(), "{}", s.audit_subject());
    }
    assert!(!BrokerSubject::User(Uuid::nil()).shared_session());
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
        shared_session: false,
    }
}
