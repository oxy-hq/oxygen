//! Unit tests for the credential cache and the touch throttle.

use super::*;
use crate::token::credential::StoredKind;

fn identity() -> Identity {
    Identity {
        user_id: Some(Uuid::new_v4()),
        email: "ada@acme.com".into(),
        name: Some("Ada".into()),
        picture: None,
    }
}

fn credential(token_id: Uuid) -> CredentialContext {
    CredentialContext {
        token_id,
        kind: StoredKind::Personal,
        principal_user_id: Uuid::new_v4(),
        all_access: true,
        platform: true,
        partner: true,
        name: "ci".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        legacy_api_key_id: None,
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: None,
        grants: Vec::new(),
        app_publish: Vec::new(),
        app_sandbox: Vec::new(),
    }
}

#[test]
fn a_fresh_entry_hits() {
    let mut cache = CredentialCache::default();
    let t0 = Instant::now();
    let id = Uuid::new_v4();
    cache.put(vec![1], identity(), credential(id), None, t0);
    let (_, cred) = cache
        .get(&[1], t0 + Duration::from_secs(29), Utc::now())
        .expect("hit");
    assert_eq!(cred.token_id, id);
}

#[test]
fn an_entry_older_than_the_ttl_misses() {
    let mut cache = CredentialCache::default();
    let t0 = Instant::now();
    cache.put(vec![1], identity(), credential(Uuid::new_v4()), None, t0);
    assert!(cache.get(&[1], t0 + TTL, Utc::now()).is_none());
    // And it is gone, not merely skipped.
    assert!(cache.get(&[1], t0, Utc::now()).is_none());
}

#[test]
fn an_entry_whose_token_has_expired_misses_inside_the_ttl() {
    let mut cache = CredentialCache::default();
    let t0 = Instant::now();
    let expires = Utc::now() + chrono::Duration::seconds(5);
    cache.put(
        vec![1],
        identity(),
        credential(Uuid::new_v4()),
        Some(expires),
        t0,
    );
    assert!(cache.get(&[1], t0, Utc::now()).is_some());
    assert!(
        cache
            .get(&[1], t0, expires + chrono::Duration::seconds(1))
            .is_none()
    );
}

#[test]
fn invalidate_drops_every_entry_for_the_token() {
    let mut cache = CredentialCache::default();
    let t0 = Instant::now();
    let id = Uuid::new_v4();
    let other = Uuid::new_v4();
    cache.put(vec![1], identity(), credential(id), None, t0);
    cache.put(vec![2], identity(), credential(other), None, t0);
    cache.invalidate(id);
    assert!(cache.get(&[1], t0, Utc::now()).is_none());
    assert!(cache.get(&[2], t0, Utc::now()).is_some());
}

#[test]
fn the_map_is_bounded() {
    let mut cache = CredentialCache::default();
    let t0 = Instant::now();
    for i in 0..(MAX_ENTRIES + 5) {
        cache.put(
            (i as u64).to_le_bytes().to_vec(),
            identity(),
            credential(Uuid::new_v4()),
            None,
            t0,
        );
    }
    assert!(cache.entries.len() <= MAX_ENTRIES);
}

#[test]
fn touch_is_claimed_once_per_interval() {
    let mut cache = CredentialCache::default();
    let t0 = Instant::now();
    let id = Uuid::new_v4();
    assert!(cache.claim_touch(id, t0));
    assert!(!cache.claim_touch(id, t0 + Duration::from_secs(60)));
    assert!(cache.claim_touch(id, t0 + TOUCH_INTERVAL));
    // Another token is independent.
    assert!(cache.claim_touch(Uuid::new_v4(), t0));
}

#[test]
fn clear_forgets_entries_and_touches() {
    let mut cache = CredentialCache::default();
    let t0 = Instant::now();
    let id = Uuid::new_v4();
    cache.put(vec![1], identity(), credential(id), None, t0);
    assert!(cache.claim_touch(id, t0));
    cache.clear();
    assert!(cache.get(&[1], t0, Utc::now()).is_none());
    assert!(cache.claim_touch(id, t0));
}
