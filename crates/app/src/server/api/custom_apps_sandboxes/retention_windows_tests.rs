//! The three retention windows (`retention::beyond_windows`): a draft never
//! spends a rollback slot, a sandbox publish never spends a draft's, and
//! nothing that was kept under two windows is pruned under three.

use std::collections::HashSet;

use uuid::Uuid;

use super::{beyond_retention, beyond_windows};

const KEEP: usize = 10;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn ids(range: std::ops::Range<u128>) -> Vec<Uuid> {
    range.map(id).collect()
}

/// `groups` oldest group first, each oldest build first, as one list newest
/// first — the order `gc_builds` reads the app's builds in.
fn newest_first(groups: &[&[Uuid]]) -> Vec<Uuid> {
    groups
        .iter()
        .flat_map(|group| group.iter().copied())
        .rev()
        .collect()
}

fn set(builds: &[Uuid]) -> HashSet<Uuid> {
    builds.iter().copied().collect()
}

/// Three builds production ran, then any number of drafts: every one of the
/// three is still there to roll back to. Under the two windows this replaces,
/// the tenth draft pruned the oldest of them and the twelfth the last.
#[test]
fn no_number_of_drafts_prunes_a_build_production_served() {
    let served = ids(1..4);
    for drafts in [10, 12, 40, 500] {
        let drafts = ids(100..100 + drafts);
        let builds = newest_first(&[&served, &drafts]);
        let beyond = beyond_windows(&builds, &HashSet::new(), &set(&served), KEEP);
        assert!(
            served.iter().all(|build| !beyond.contains(build)),
            "{} drafts pruned a rollback target",
            drafts.len()
        );
        // The drafts still prune their own oldest: all but the newest ten.
        assert_eq!(beyond.len(), drafts.len() - KEEP);
        assert!(beyond.iter().all(|build| drafts.contains(build)));
    }
    // What the shared window did with twelve drafts: every rollback target gone.
    let builds = newest_first(&[&served, &ids(100..112)]);
    let shared = beyond_retention(&builds, &HashSet::new(), KEEP);
    assert!(served.iter().all(|build| shared.contains(build)));
}

/// The production window is a window: the eleventh build production served
/// goes, oldest first, and no draft or sandbox build goes with it.
#[test]
fn the_production_window_keeps_its_newest_and_drops_its_own_oldest() {
    let served = ids(1..13); // twelve
    let drafts = ids(100..103);
    let sandbox = ids(200..203);
    let builds = newest_first(&[&served, &drafts, &sandbox]);
    let beyond = beyond_windows(&builds, &set(&sandbox), &set(&served), KEEP);
    assert_eq!(beyond, vec![id(2), id(1)], "newest first, its own oldest");
}

/// A sandbox's window is what it was: ten sandbox builds and ten drafts after
/// a promote prune nothing, and the eleventh sandbox build prunes the oldest
/// sandbox build alone.
#[test]
fn the_sandbox_window_is_untouched() {
    let served = ids(1..3);
    let drafts = ids(100..110);
    let sandbox = ids(200..210);
    let builds = newest_first(&[&served, &drafts, &sandbox]);
    assert!(beyond_windows(&builds, &set(&sandbox), &set(&served), KEEP).is_empty());

    let sandbox = ids(200..211);
    let builds = newest_first(&[&served, &drafts, &sandbox]);
    assert_eq!(
        beyond_windows(&builds, &set(&sandbox), &set(&served), KEEP),
        vec![id(200)]
    );
    // With no build production served, three windows are the two there were.
    for keep in [1, 2, 10] {
        assert_eq!(
            beyond_windows(&builds, &set(&sandbox), &HashSet::new(), keep),
            beyond_retention(&builds, &set(&sandbox), keep),
            "keep {keep}"
        );
    }
}

/// Every publisher: whatever the mix and the order, a build the two windows
/// kept is kept by the three. The change only ever retains more.
#[test]
fn nothing_kept_before_is_pruned_now() {
    // Builds 0..60 by age; a build's window is fixed by its number.
    let all = ids(0..60);
    for (sandbox_every, served_every) in [(2, 3), (3, 2), (5, 7), (7, 5), (61, 1), (1, 61)] {
        let sandbox: Vec<Uuid> = (0..60u128)
            .filter(|n| n % sandbox_every == 0 && n % served_every != 0)
            .map(id)
            .collect();
        let served: Vec<Uuid> = (0..60u128)
            .filter(|n| n % served_every == 0)
            .map(id)
            .collect();
        let builds = newest_first(&[&all]);
        let before = beyond_retention(&builds, &set(&sandbox), KEEP);
        let after = beyond_windows(&builds, &set(&sandbox), &set(&served), KEEP);
        assert!(
            after.iter().all(|build| before.contains(build)),
            "sandbox every {sandbox_every}, served every {served_every}"
        );
    }
}
