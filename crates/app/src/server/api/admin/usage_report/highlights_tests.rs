use super::super::model::fixtures::*;
use super::*;
use chrono::Duration;

fn kinds_of(app: AppUsage) -> Vec<Kind> {
    highlights(&snapshot(vec![org(1, "Poke House", vec![app])]))
        .into_iter()
        .map(|h| h.kind)
        .collect()
}

fn detail_of(app: AppUsage) -> String {
    highlights(&snapshot(vec![org(1, "Poke House", vec![app])]))
        .remove(0)
        .detail
}

#[test]
fn an_app_two_people_used_and_nobody_opened_went_quiet() {
    assert_eq!(kinds_of(app(1, "Store Ops", 0, 2)), [Kind::WentQuiet]);
    assert_eq!(detail_of(app(1, "Store Ops", 0, 6)), "6 people → nobody");
    // One person not coming back is not news.
    assert!(kinds_of(app(1, "Store Ops", 0, 1)).is_empty());
}

#[test]
fn a_fall_is_called_out_when_three_people_left_and_half_are_gone() {
    assert_eq!(kinds_of(app(1, "Store Ops", 3, 9)), [Kind::Dropping]);
    assert_eq!(detail_of(app(1, "Store Ops", 3, 9)), "9 → 3 people");
    // Exactly half is a fall.
    assert_eq!(kinds_of(app(1, "Store Ops", 3, 6)), [Kind::Dropping]);
    // Three left, but most stayed.
    assert!(kinds_of(app(1, "Store Ops", 7, 10)).is_empty());
    // Half are gone, but that is two people.
    assert!(kinds_of(app(1, "Store Ops", 2, 4)).is_empty());
}

#[test]
fn a_rise_is_called_out_when_three_people_came_and_that_is_half_again() {
    assert_eq!(kinds_of(app(1, "Store Ops", 9, 6)), [Kind::Growing]);
    assert_eq!(detail_of(app(1, "Store Ops", 9, 6)), "6 → 9 people");
    // Three more, but on a base of ten.
    assert!(kinds_of(app(1, "Store Ops", 13, 10)).is_empty());
    // Doubled, but that is two people.
    assert!(kinds_of(app(1, "Store Ops", 4, 2)).is_empty());
}

#[test]
fn a_first_week_is_good_news_and_not_a_rise_from_nothing() {
    let mut new = app(1, "Store Ops", 4, 0);
    new.first_week = true;
    assert_eq!(kinds_of(new.clone()), [Kind::FirstWeek]);
    assert_eq!(detail_of(new), "4 people");
    // Back in use after a gap is neither a first week nor a rise.
    assert!(kinds_of(app(1, "Store Ops", 4, 0)).is_empty());
}

#[test]
fn an_app_is_unused_only_after_two_full_weeks_live() {
    assert_eq!(kinds_of(app(1, "Store Ops", 0, 0)), [Kind::Unused]);

    let mut fresh = app(1, "Store Ops", 0, 0);
    fresh.published_at = Some(period().previous().start + Duration::hours(1));
    assert!(kinds_of(fresh).is_empty(), "published during the two weeks");

    let mut draft = app(1, "Store Ops", 0, 0);
    draft.published_at = None;
    assert!(kinds_of(draft).is_empty(), "not live");
}

#[test]
fn functions_fail_at_one_in_ten_or_at_five_that_are_half() {
    let with = |calls, failed| {
        let mut a = app(1, "Store Ops", 5, 5);
        a.current.function_calls = calls;
        a.current.function_failures = failed;
        a
    };
    assert_eq!(kinds_of(with(20, 2)), [Kind::FailingFunctions]);
    assert_eq!(detail_of(with(200, 31)), "31 of 200 calls failed");
    assert!(kinds_of(with(20, 1)).is_empty(), "one in twenty");
    assert!(
        kinds_of(with(19, 2)).is_empty(),
        "too few calls for a ratio"
    );
    assert_eq!(kinds_of(with(8, 5)), [Kind::FailingFunctions]);
    assert!(kinds_of(with(8, 4)).is_empty(), "half, but only four");
    assert!(kinds_of(with(0, 0)).is_empty());
}

#[test]
fn page_errors_need_three_sessions_and_a_quarter_of_them() {
    let with = |sessions, errored| {
        let mut a = app(1, "Store Ops", 5, 5);
        a.current.sessions = sessions;
        a.current.error_sessions = errored;
        a
    };
    assert_eq!(kinds_of(with(12, 3)), [Kind::ClientErrors]);
    assert_eq!(detail_of(with(12, 3)), "3 of 12 visits hit an error");
    assert!(kinds_of(with(13, 3)).is_empty(), "under a quarter");
    assert!(kinds_of(with(4, 2)).is_empty(), "two sessions");
    // More errors than recorded sessions: the errors are the denominator.
    assert_eq!(detail_of(with(1, 4)), "4 of 4 visits hit an error");
}

#[test]
fn an_app_can_be_growing_and_failing_at_once() {
    let mut a = app(1, "Store Ops", 9, 6);
    a.current.function_calls = 40;
    a.current.function_failures = 20;
    assert_eq!(kinds_of(a), [Kind::FailingFunctions, Kind::Growing]);
}

#[test]
fn attention_comes_first_and_the_biggest_of_a_kind_leads() {
    let report = snapshot(vec![
        org(
            1,
            "Poke House",
            vec![
                app(10, "Checklists", 9, 6),
                app(11, "Store Ops", 0, 3),
                app(12, "Archive", 0, 0),
            ],
        ),
        org(2, "Rivermark", vec![app(20, "Crew Board", 0, 8)]),
    ]);
    let found = highlights(&report);
    let order: Vec<(&str, Kind)> = found
        .iter()
        .map(|h| (h.app_name.as_str(), h.kind))
        .collect();
    assert_eq!(
        order,
        [
            ("Crew Board", Kind::WentQuiet),
            ("Store Ops", Kind::WentQuiet),
            ("Checklists", Kind::Growing),
            ("Archive", Kind::Unused),
        ]
    );
    assert_eq!(found[0].org_name, "Rivermark");
    assert_eq!(found[0].tone, Tone::Attention);
    assert_eq!(found[3].tone, Tone::Idle);
}

#[test]
fn a_highlight_serializes_its_kind_and_tone_as_the_console_reads_them() {
    let found = highlights(&snapshot(vec![org(
        1,
        "Poke House",
        vec![app(10, "Store Ops", 0, 3)],
    )]));
    let json = serde_json::to_value(&found[0]).unwrap();
    assert_eq!(json["kind"], "went_quiet");
    assert_eq!(json["tone"], "attention");
    assert_eq!(json["label"], "Went quiet");
    assert_eq!(json["detail"], "3 people → nobody");
    assert_eq!(json["app_slug"], "store-ops");
    assert!(json.get("weight").is_none());
}
