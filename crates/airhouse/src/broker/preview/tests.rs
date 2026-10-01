//! I1, fence 3: a preview credential writes only the preview's own schemas.

use std::time::{Duration, Instant};

use chrono::Utc;
use uuid::Uuid;

use super::super::{AirhouseTokenBroker, BrokerError, BrokerSubject, CacheEntry};
use crate::admin::{AirhouseAdminClient, EphemeralCredential, UserRole};
use crate::preview_sql::PreviewNamespace;

const KEY: &str = "feat_je_v2_92a1b7";
pub(super) const TTL: Duration = Duration::from_secs(900);

pub(super) fn ns() -> PreviewNamespace {
    PreviewNamespace::from_key(KEY).unwrap()
}

pub(super) fn schema(live: &str) -> String {
    format!("preview_{KEY}__{live}")
}

/// A broker whose Admin API is nowhere: a test that reaches the network or the
/// tenant lookup fails with some other error, never the one it asserts.
fn broker() -> AirhouseTokenBroker {
    AirhouseTokenBroker::new(AirhouseAdminClient::new("http://127.0.0.1:9", "tok"))
}

/// Record what the capabilities probe would have answered, `age` ago.
pub(super) async fn scopes_writers(broker: &AirhouseTokenBroker, supported: bool, age: Duration) {
    let at = Instant::now()
        .checked_sub(age)
        .expect("a monotonic clock that old");
    *broker.scoped_writers.known.lock().unwrap() = Some((supported, at));
}

pub(super) fn preview_subject(ws: Uuid, schemas: &[String]) -> BrokerSubject {
    BrokerSubject::Preview {
        workspace_id: ws,
        preview_key: KEY.into(),
        schemas: schemas.to_vec(),
    }
}

pub(super) fn cred(role: &str, write_schemas: Option<Vec<String>>) -> EphemeralCredential {
    EphemeralCredential {
        username: "eph_preview".into(),
        password: "tk_preview".into(),
        tenant: "acme".into(),
        role: role.into(),
        expires_at: Utc::now() + chrono::Duration::seconds(3600),
        service_account_id: "sa_acme".into(),
        write_schemas,
    }
}

/// Put `cred` in the cache where `mint_for_preview(scope, role)` looks first,
/// so the call answers from it without the network.
pub(super) async fn seed(
    broker: &AirhouseTokenBroker,
    ws: Uuid,
    scope: &[String],
    role: UserRole,
    cred: EphemeralCredential,
) {
    let key = broker.cache_key(ws, &preview_subject(ws, scope), role);
    broker.cache.write().await.insert(key, CacheEntry { cred });
}

#[test]
fn preview_subject_scopes_writes_to_its_schemas_only() {
    let ws = Uuid::nil();
    let (a, b) = (schema("toast_pos"), schema("site_selection"));
    let subject = preview_subject(ws, &[a.clone(), b.clone()]);

    assert_eq!(
        subject.audit_subject(),
        format!("system:workspace:{ws}:preview:{KEY}")
    );
    assert_eq!(
        subject.write_schemas(UserRole::Writer),
        Some(vec![a.clone(), b.clone()])
    );
    assert_eq!(subject.write_schemas(UserRole::Reader), None);

    // Two scopes, two cache entries, and neither is the Reader's.
    let broker = broker();
    let both = broker.cache_key(ws, &subject, UserRole::Writer);
    let one = broker.cache_key(
        ws,
        &preview_subject(ws, std::slice::from_ref(&a)),
        UserRole::Writer,
    );
    assert_ne!(both, one);
    assert_ne!(both, broker.cache_key(ws, &subject, UserRole::Reader));
    // Never the entry of another subject in the same workspace.
    let system = BrokerSubject::System {
        workspace_id: ws,
        purpose: super::super::SystemPurpose::Preview,
    };
    assert_ne!(both, broker.cache_key(ws, &system, UserRole::Writer));
}

#[tokio::test]
async fn mint_for_preview_refuses_a_schema_outside_its_key() {
    let broker = broker();
    let ws = Uuid::new_v4();
    let other = PreviewNamespace::for_branch(ws, "someone-elses-branch");
    let refused = [
        (vec!["toast_pos".to_string()], UserRole::Writer),
        (
            vec![schema("toast_pos"), "bookkeeping".into()],
            UserRole::Writer,
        ),
        (
            vec![format!("{}toast_pos", other.prefix())],
            UserRole::Writer,
        ),
        (vec![format!("PREVIEW_{KEY}__toast_pos")], UserRole::Writer),
        (vec![schema("a__b")], UserRole::Writer),
        (vec![schema("preview_x")], UserRole::Writer),
        (vec![schema(&"x".repeat(40))], UserRole::Writer),
        (vec![], UserRole::Writer),
        (vec![schema("toast_pos")], UserRole::Reader),
        (vec![schema("toast_pos")], UserRole::Admin),
        (vec![], UserRole::Admin),
    ];
    for (schemas, role) in refused {
        let err = broker
            .mint_for_preview(ws, &ns(), &schemas, role, TTL)
            .await
            .expect_err(&format!("{role:?} {schemas:?} was minted"));
        assert!(
            matches!(err, BrokerError::PreviewScope(_)),
            "{role:?} {schemas:?}: refused for the wrong reason: {err}"
        );
    }

    // The control: an owned scope gets past the check to the mint itself,
    // which fails here only because there is no Airhouse or tenant to ask.
    let err = broker
        .mint_for_preview(ws, &ns(), &[schema("toast_pos")], UserRole::Writer, TTL)
        .await
        .expect_err("no Airhouse to mint from");
    assert!(!matches!(err, BrokerError::PreviewScope(_)), "{err}");
}

#[tokio::test]
async fn preview_writes_refuse_an_unscoped_writer() {
    let broker = broker();
    let ws = Uuid::new_v4();
    let scope = vec![schema("site_selection"), schema("toast_pos")];
    let unscoped_echoes = [
        None,
        Some(vec![schema("toast_pos")]),
        Some(vec![
            schema("site_selection"),
            schema("toast_pos"),
            "bookkeeping".into(),
        ]),
        Some(vec!["toast_pos".to_string(), "site_selection".to_string()]),
    ];
    for echo in unscoped_echoes {
        // Reasserted each iteration: an `UnscopedWriter` un-keeps a cached
        // "yes" (that is what this module's other tests check), so without
        // this every echo after the first would instead see
        // `ScopedWritersUnsupported` — a real effect, just not the one this
        // loop means to exercise.
        scopes_writers(&broker, true, Duration::ZERO).await;
        seed(
            &broker,
            ws,
            &scope,
            UserRole::Writer,
            cred("writer", echo.clone()),
        )
        .await;
        // Named out of order: the broker sorts before it looks.
        let asked = vec![scope[1].clone(), scope[0].clone()];
        let err = broker
            .mint_for_preview(ws, &ns(), &asked, UserRole::Writer, TTL)
            .await
            .expect_err(&format!("a Writer echoing {echo:?} was handed out"));
        assert!(
            matches!(&err, BrokerError::UnscopedWriter { asked, echoed }
                if *asked == scope && *echoed == echo),
            "{err}"
        );
        // Refused credentials are forgotten, not served to the next caller.
        let key = broker.cache_key(ws, &preview_subject(ws, &scope), UserRole::Writer);
        assert!(!broker.cache.read().await.contains_key(&key));
    }

    // The controls: the exact echo is a Writer the preview may use, and a
    // Reader carries no scope to echo. Reasserted for the same reason as in
    // the loop: the last iteration's `UnscopedWriter` un-kept the "yes".
    scopes_writers(&broker, true, Duration::ZERO).await;
    seed(
        &broker,
        ws,
        &scope,
        UserRole::Writer,
        cred("writer", Some(scope.clone())),
    )
    .await;
    let ok = broker
        .mint_for_preview(ws, &ns(), &scope, UserRole::Writer, TTL)
        .await
        .expect("a Writer scoped as asked");
    assert_eq!(ok.write_schemas.as_deref(), Some(scope.as_slice()));
    seed(&broker, ws, &[], UserRole::Reader, cred("reader", None)).await;
    broker
        .mint_for_preview(ws, &ns(), &[], UserRole::Reader, TTL)
        .await
        .expect("a Reader");

    // A "Reader" that came back as anything else is refused too.
    seed(&broker, ws, &[], UserRole::Reader, cred("writer", None)).await;
    let err = broker
        .mint_for_preview(ws, &ns(), &[], UserRole::Reader, TTL)
        .await
        .expect_err("a Writer handed out as a Reader");
    assert!(matches!(err, BrokerError::PreviewScope(_)), "{err}");
}

/// Every entry point that takes a subject runs the same checks, so a
/// hand-built preview subject cannot skip them.
#[tokio::test]
async fn a_hand_built_preview_subject_is_checked_too() {
    let broker = broker();
    let ws = Uuid::new_v4();
    let subject = preview_subject(ws, &["toast_pos".to_string()]);
    let err = broker
        .evict_and_remint(ws, subject, UserRole::Writer, TTL)
        .await
        .expect_err("a live schema in a preview scope");
    assert!(matches!(err, BrokerError::PreviewScope(_)), "{err}");
}

/// A reordered scope is refused before the cache is touched: the valid
/// credential the sorted scope shares is neither handed out nor evicted.
#[tokio::test]
async fn a_reordered_scope_never_evicts_the_shared_credential() {
    let broker = broker();
    scopes_writers(&broker, true, Duration::ZERO).await;
    let ws = Uuid::new_v4();
    let scope = vec![schema("site_selection"), schema("toast_pos")];
    let valid = cred("writer", Some(scope.clone()));
    seed(&broker, ws, &scope, UserRole::Writer, valid).await;
    let reversed = preview_subject(ws, &[scope[1].clone(), scope[0].clone()]);
    let err = broker
        .evict_and_remint(ws, reversed, UserRole::Writer, TTL)
        .await
        .expect_err("an unsorted scope");
    assert!(matches!(err, BrokerError::PreviewScope(_)), "{err}");
    let key = broker.cache_key(ws, &preview_subject(ws, &scope), UserRole::Writer);
    assert!(broker.cache.read().await.contains_key(&key));
}

/// An Airhouse that cannot scope Writers gets no preview Writer mint at all,
/// not even a cached one; Readers are unaffected. A "no" is asked again once
/// it is old, so an upgrade is noticed.
#[tokio::test]
async fn a_writer_is_refused_before_minting_where_airhouse_cannot_scope() {
    let broker = broker();
    let ws = Uuid::new_v4();
    let scope = vec![schema("toast_pos")];
    seed(
        &broker,
        ws,
        &scope,
        UserRole::Writer,
        cred("writer", Some(scope.clone())),
    )
    .await;
    seed(&broker, ws, &[], UserRole::Reader, cred("reader", None)).await;
    scopes_writers(&broker, false, Duration::ZERO).await;
    let err = broker
        .mint_for_preview(ws, &ns(), &scope, UserRole::Writer, TTL)
        .await
        .expect_err("a Writer where Airhouse cannot scope one");
    assert!(
        matches!(err, BrokerError::ScopedWritersUnsupported),
        "{err}"
    );
    broker
        .mint_for_preview(ws, &ns(), &[], UserRole::Reader, TTL)
        .await
        .expect("a Reader needs no scope");

    // Past the recheck interval the probe runs again (and here finds nothing
    // to ask, which is an error, not a yes).
    scopes_writers(&broker, false, super::RECHECK_UNSUPPORTED).await;
    let err = broker
        .mint_for_preview(ws, &ns(), &scope, UserRole::Writer, TTL)
        .await
        .expect_err("no Airhouse to ask");
    assert!(matches!(err, BrokerError::Airhouse(_)), "{err}");
}
