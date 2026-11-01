//! `for`: who a widgets group or a copy is for.

use serde_json::{Value, json};

use super::layout::{Drawn, Viewer, check, effective, read, resolve, visible, without};
use super::layout_tests::catalog;

fn viewer(email: &str, teams: &[&str]) -> Viewer {
    Viewer::new(Some(email), teams.iter().map(|t| t.to_string()))
}

fn problems(doc: Value) -> Vec<(String, String)> {
    check(&doc, &catalog())
        .expect_err("refused")
        .into_iter()
        .map(|p| (p.path, p.message))
        .collect()
}

fn paths(doc: Value) -> Vec<String> {
    problems(doc).into_iter().map(|(p, _)| p).collect()
}

/// A layout with `group_for` on the group and `item_for` on bookkeeping/close.
fn layout_with(group_for: Value, item_for: Value) -> Value {
    json!({ "version": 1, "sections": [
        { "kind": "widgets", "for": group_for, "items": [
            { "widget": "bookkeeping/close", "for": item_for },
            { "widget": "store-ops/today" }
        ] }
    ] })
}

/// The keys one viewer is shown from `doc`.
fn keys_for(doc: &Value, who: &Viewer) -> Vec<String> {
    let cat = catalog();
    let layout = check(doc, &cat).expect("valid");
    visible(resolve(Some(&layout), &cat), who)
        .iter()
        .flat_map(|d| match d {
            Drawn::Widgets { placed, .. } => placed.iter().map(|p| p.key(&cat)).collect(),
            _ => vec![],
        })
        .collect()
}

#[test]
fn a_for_is_read_normalised_and_deduped() {
    let doc = layout_with(
        json!({ "people": [" Scott@Poke.House ", "scott@poke.house"], "teams": ["Office", "office"] }),
        json!({ "people": ["dylan@poke.house"] }),
    );
    let layout = check(&doc, &catalog()).expect("valid");
    let eff = effective(&resolve(Some(&layout), &catalog()), &catalog());
    assert_eq!(
        eff["sections"][0]["for"],
        json!({ "people": ["scott@poke.house"], "teams": ["office"] })
    );
    assert_eq!(
        eff["sections"][0]["items"][0]["for"],
        json!({ "people": ["dylan@poke.house"] })
    );
    assert!(eff["sections"][0]["items"][1].get("for").is_none());
}

#[test]
fn for_round_trips_through_the_effective_layout() {
    let doc = layout_with(
        json!({ "teams": ["Office"] }),
        json!({ "people": ["scott@poke.house"] }),
    );
    let layout = check(&doc, &catalog()).unwrap();
    let drawn = resolve(Some(&layout), &catalog());
    let eff = effective(&drawn, &catalog());
    let again = check(&eff, &catalog()).expect("the effective layout is valid");
    // The unlisted warehouse widget moves to a group of its own, ungated, so
    // writing the document back does not narrow it to the first group's `for`.
    assert_eq!(
        eff["sections"][1]["items"][0]["widget"],
        "warehouse/receiving"
    );
    assert!(eff["sections"][1].get("for").is_none());
    for who in [
        viewer("scott@poke.house", &[]),
        viewer("amy@poke.house", &["Office"]),
        viewer("amy@poke.house", &[]),
    ] {
        assert_eq!(
            keys_for(&eff, &who),
            keys_for(&doc, &who),
            "{who:?} sees the same home after a write-back"
        );
    }
    assert_eq!(
        effective(&resolve(Some(&again), &catalog()), &catalog()),
        eff
    );
    // And a stored layout still reads.
    assert!(read(&doc).is_some());
}

#[test]
fn every_for_problem_is_listed_with_its_path() {
    let got = paths(json!({ "version": 1, "sections": [
        { "kind": "widgets", "for": "scott@poke.house" },
        { "kind": "widgets", "for": {} },
        { "kind": "widgets", "for": { "people": [] } },
        { "kind": "widgets", "for": { "people": ["scott"], "teams": ["  "], "roles": ["x"] } },
        { "kind": "widgets", "for": { "people": "scott@poke.house", "teams": [7] } },
        { "kind": "widgets", "items": [
            { "widget": "store-ops/today", "for": { "people": [3] } },
            { "widget": "warehouse/receiving", "for": [] }
        ] }
    ] }));
    for want in [
        "sections[0].for",
        "sections[1].for",
        "sections[2].for",
        "sections[3].for.roles",
        "sections[3].for.people[0]",
        "sections[3].for.teams[0]",
        "sections[4].for.people",
        "sections[4].for.teams[0]",
        "sections[5].items[0].for.people[0]",
        "sections[5].items[1].for",
    ] {
        assert!(got.iter().any(|p| p == want), "missing {want} in {got:?}");
    }
}

#[test]
fn a_for_may_list_at_most_fifty_of_each() {
    let people: Vec<String> = (0..51).map(|i| format!("p{i}@poke.house")).collect();
    let got = paths(layout_with(
        json!({ "people": people }),
        json!({ "teams": ["x"] }),
    ));
    assert_eq!(got, ["sections[0].for.people"]);
    let fifty: Vec<String> = (0..50).map(|i| format!("p{i}@poke.house")).collect();
    assert!(
        check(
            &layout_with(json!({ "people": fifty }), json!({ "teams": ["x"] })),
            &catalog()
        )
        .is_ok()
    );
}

#[test]
fn a_for_problem_says_how_to_fix_it() {
    let got = check(
        &layout_with(json!({}), json!({ "teams": ["x"] })),
        &catalog(),
    )
    .unwrap_err();
    assert_eq!(got[0].path, "sections[0].for");
    assert!(got[0].fix.contains("leave `for` out"), "{}", got[0].fix);
}

#[test]
fn no_for_is_everyone() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "items": [{ "widget": "bookkeeping/close" }] }
    ] });
    assert_eq!(
        keys_for(&doc, &viewer("nobody@poke.house", &[])).len(),
        3,
        "the listed copy and the two others"
    );
}

#[test]
fn a_person_or_a_team_member_is_admitted_case_insensitively() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "for": { "people": ["Scott@Poke.House"], "teams": ["Office"] },
          "others": "hide",
          "items": [{ "widget": "bookkeeping/close" }] }
    ] });
    let seen = |v: Viewer| keys_for(&doc, &v);
    assert_eq!(seen(viewer("SCOTT@poke.house", &[])), ["bookkeeping/close"]);
    assert_eq!(
        seen(viewer("amy@poke.house", &["OFFICE"])),
        ["bookkeeping/close"]
    );
    assert!(seen(viewer("amy@poke.house", &["Stores"])).is_empty());
    assert!(seen(Viewer::default()).is_empty());
    // A frontline worker with no mailbox is reached through a team.
    assert_eq!(
        seen(Viewer::new(None, ["office".to_string()])),
        ["bookkeeping/close"]
    );
}

#[test]
fn an_item_for_narrows_its_groups_for() {
    let doc = layout_with(
        json!({ "people": ["scott@poke.house", "dylan@poke.house"] }),
        json!({ "people": ["scott@poke.house", "amy@poke.house"] }),
    );
    // Both admit Scott; only the group admits Dylan; only the item admits Amy,
    // and the group does not.
    assert_eq!(
        keys_for(&doc, &viewer("scott@poke.house", &[])),
        [
            "bookkeeping/close",
            "store-ops/today",
            "warehouse/receiving"
        ]
    );
    assert_eq!(
        keys_for(&doc, &viewer("dylan@poke.house", &[])),
        ["store-ops/today", "warehouse/receiving"]
    );
    assert_eq!(
        keys_for(&doc, &viewer("amy@poke.house", &[])),
        ["warehouse/receiving"],
        "the unlisted widget keeps no `for`"
    );
}

#[test]
fn widgets_the_layout_does_not_list_are_for_everyone_even_in_a_gated_first_group() {
    let doc = layout_with(
        json!({ "people": ["scott@poke.house"] }),
        json!({ "teams": ["x"] }),
    );
    let keys = keys_for(&doc, &viewer("amy@poke.house", &[]));
    // Not the two listed (the group excludes Amy), but the unlisted warehouse
    // widget keeps no `for`.
    assert_eq!(keys, ["warehouse/receiving"]);
}

#[test]
fn a_group_the_filter_empties_is_omitted() {
    let doc = json!({ "version": 1, "sections": [
        { "kind": "widgets", "title": "Back office", "for": { "teams": ["Office"] },
          "others": "hide", "items": [{ "widget": "bookkeeping/close" }] },
        { "kind": "widgets", "title": "Stores", "items": [] },
        { "kind": "apps" }
    ] });
    let cat = catalog();
    let layout = check(&doc, &cat).unwrap();
    let drawn = resolve(Some(&layout), &cat);
    let titles = |d: &[Drawn]| -> Vec<Option<String>> {
        d.iter()
            .filter_map(|d| match d {
                Drawn::Widgets { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        titles(&drawn),
        [Some("Back office".into()), Some("Stores".into())]
    );
    let theirs = visible(drawn.clone(), &viewer("amy@poke.house", &[]));
    // The gated group is gone; the group that was already empty stays as it was.
    assert_eq!(titles(&theirs), [Some("Stores".into())]);
    assert!(theirs.contains(&Drawn::Apps));
    assert_eq!(
        titles(&visible(drawn, &viewer("amy@poke.house", &["office"]))).len(),
        2
    );
}

#[test]
fn without_drops_named_copies_and_the_groups_they_empty() {
    let cat = catalog();
    let drawn = resolve(None, &cat);
    let kept = without(drawn.clone(), |p| p.index == 1);
    let Drawn::Widgets { placed, .. } = &kept[0] else {
        panic!("widgets");
    };
    assert_eq!(placed.iter().map(|p| p.index).collect::<Vec<_>>(), [0, 2]);
    let none = without(drawn, |_| true);
    assert!(none.iter().all(|d| !matches!(d, Drawn::Widgets { .. })));
}
