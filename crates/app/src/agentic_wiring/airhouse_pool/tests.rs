//! Pool eviction policy.

use super::{Budgets, keys_to_evict, keys_to_evict_by_budget};

fn e(k: &str, last: u64) -> (String, u64) {
    (k.to_string(), last)
}

#[test]
fn app_identities_and_the_analytics_path_do_not_evict_each_other() {
    let entries = [
        e("mgd:w:u1:Reader", 990),
        e("mgd:w:u2:Reader", 995),
        e("app:w:store-ops", 900),
        e("app:w:bookkeeping", 910),
    ];
    let budgets = Budgets {
        idle_secs: 600,
        max_identities: 3,
        max_app_identities: 3,
        max_preview_identities: 3,
    };
    // Four identities against one shared cap of 3 would evict the oldest two
    // — both apps. Budgeted apart, each side is under its own cap.
    assert!(keys_to_evict(&entries, 1000, 600, 3).len() == 2);
    assert!(keys_to_evict_by_budget(&entries, 1000, budgets).is_empty());
}

#[test]
fn preview_identities_have_their_own_budget() {
    let entries = [
        e("mgd:w:u1:Reader", 800),
        e("mgd:w:u2:Reader", 810),
        e("preview:w:feat_x_abc123", 900),
        e(
            "preview:w:feat_x_abc123:preview_feat_x_abc123__toast_pos",
            910,
        ),
        e("preview:w:feat_x_abc123:preview_feat_x_abc123__site", 920),
    ];
    let budgets = Budgets {
        idle_secs: 600,
        max_identities: 3,
        max_app_identities: 3,
        max_preview_identities: 3,
    };
    // A preview writing two schemas beside its reader is three identities; it
    // makes room within its own budget and never evicts the analytics path's.
    assert_eq!(
        keys_to_evict_by_budget(&entries, 1000, budgets),
        vec!["preview:w:feat_x_abc123".to_string()]
    );
    // Under one shared cap, the two (older) analytics identities would go.
    let shared = keys_to_evict(&entries, 1000, 600, 3);
    assert!(
        shared.contains(&"mgd:w:u1:Reader".to_string()),
        "{shared:?}"
    );
    assert!(
        shared.contains(&"mgd:w:u2:Reader".to_string()),
        "{shared:?}"
    );
}

#[test]
fn evicts_only_idle_entries() {
    let entries = [e("a", 100), e("b", 950), e("c", 970)];
    // now=1000, idle=60 → "a" (idle 900) evicted; b (idle 50) and c (idle 30) kept.
    let mut out = keys_to_evict(&entries, 1000, 60, 100);
    out.sort();
    assert_eq!(out, vec!["a".to_string()]);
}

#[test]
fn idle_secs_is_inclusive_boundary() {
    let entries = [e("a", 940)];
    assert_eq!(
        keys_to_evict(&entries, 1000, 60, 100),
        vec!["a".to_string()]
    );
}

#[test]
fn cap_evicts_lru_to_make_room() {
    // All fresh, cap=3 → drop oldest so one new identity fits.
    let entries = [e("a", 10), e("b", 20), e("c", 30)];
    assert_eq!(
        keys_to_evict(&entries, 30, 100_000, 3),
        vec!["a".to_string()]
    );
}

#[test]
fn cap_zero_disables_cap_eviction() {
    let entries = [e("a", 10), e("b", 20)];
    assert!(keys_to_evict(&entries, 30, 100_000, 0).is_empty());
}

#[test]
fn under_cap_and_fresh_evicts_nothing() {
    let entries = [e("a", 10), e("b", 20)];
    assert!(keys_to_evict(&entries, 30, 100_000, 10).is_empty());
}
