//! The restore check, one test per row of its rule table.

use super::{Shipped, StoreProgress, Verdict, check};

fn store(generation: i64, intent: i64, shipped: i64) -> StoreProgress {
    StoreProgress {
        generation,
        intent,
        shipped,
    }
}

fn airhouse(generation: i64, batch: i64) -> Option<Shipped> {
    Some(Shipped { generation, batch })
}

#[test]
fn a_fresh_stream_proceeds() {
    assert_eq!(check(None, store(1, 0, 0)), Verdict::Proceed);
    // A batch declared and never written: still nothing shipped.
    assert_eq!(check(None, store(1, 3, 0)), Verdict::Proceed);
}

#[test]
fn airhouse_with_nothing_after_a_ship_is_lost() {
    assert_eq!(check(None, store(1, 4, 4)), Verdict::Lost);
}

#[test]
fn airhouse_past_the_intent_is_rewound() {
    assert_eq!(check(airhouse(1, 9), store(1, 4, 4)), Verdict::Rewound);
}

#[test]
fn airhouse_on_a_later_generation_is_rewound() {
    assert_eq!(check(airhouse(3, 4), store(2, 4, 4)), Verdict::Rewound);
}

#[test]
fn an_earlier_generation_with_nothing_shipped_since_proceeds() {
    assert_eq!(check(airhouse(1, 2), store(2, 0, 0)), Verdict::Proceed);
    // A batch declared since the bump, not yet confirmed.
    assert_eq!(check(airhouse(1, 2), store(2, 1, 0)), Verdict::Proceed);
}

#[test]
fn an_earlier_generation_after_a_ship_is_lost() {
    assert_eq!(check(airhouse(1, 2), store(2, 7, 5)), Verdict::Lost);
    // Whatever batch Airhouse holds in the earlier generation.
    assert_eq!(check(airhouse(1, 9), store(2, 4, 4)), Verdict::Lost);
}

#[test]
fn airhouse_behind_shipped_is_lost() {
    assert_eq!(check(airhouse(1, 3), store(1, 5, 5)), Verdict::Lost);
}

#[test]
fn a_crash_between_shipped_and_intent_proceeds() {
    assert_eq!(check(airhouse(1, 5), store(1, 6, 4)), Verdict::Proceed);
    assert_eq!(check(airhouse(1, 6), store(1, 6, 4)), Verdict::Proceed);
}

#[test]
fn an_earlier_generation_past_the_intent_proceeds() {
    assert_eq!(
        check(
            Some(Shipped {
                generation: 1,
                batch: 9
            }),
            StoreProgress {
                generation: 2,
                intent: 0,
                shipped: 0
            }
        ),
        Verdict::Proceed
    );
}

#[test]
fn caught_up_proceeds() {
    assert_eq!(check(airhouse(1, 4), store(1, 4, 4)), Verdict::Proceed);
    assert_eq!(check(airhouse(1, 4), store(1, 5, 4)), Verdict::Proceed);
}

#[test]
fn a_progress_row_reads_back_from_text() {
    let row = [Some("2".to_owned()), Some("17".to_owned())];
    assert_eq!(
        Shipped::from_row(&row),
        Ok(Shipped {
            generation: 2,
            batch: 17
        })
    );
    assert!(Shipped::from_row(&[Some("2".to_owned()), None]).is_err());
    assert!(Shipped::from_row(&[Some("two".to_owned()), Some("1".to_owned())]).is_err());
}

#[test]
fn a_verdict_names_itself_in_a_hold_reason() {
    assert_eq!(format!("{:?}", Verdict::Rewound), "Rewound");
    assert_eq!(format!("{:?}", Verdict::Lost), "Lost");
}
