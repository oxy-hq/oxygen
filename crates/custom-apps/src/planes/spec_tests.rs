use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::json;

use super::{
    OltpMode, Plane, PlaneSpec, SemanticChoice, SemanticMode, WorkspaceActionsMode, default_parent,
    valid_branch_name,
};

fn dev() -> AppEnvironment {
    AppEnvironment::parse("dev-a1").expect("a valid sandbox name")
}

/// The Global Constraints table: every plane's mode on staging. A `dev-*`
/// sandbox starts from the same table: it holds workspace actions as staging
/// does (`refused` is a mode it can ask for).
const TODAYS_MODES: [(&str, &str); 12] = [
    ("build", "own"),
    ("semantic", "build-pin"),
    ("reads", "live"),
    ("oltp", "schema"),
    ("airhouse", "write-only"),
    ("warehouse", "mapped"),
    ("storage", "branch"),
    ("secrets", "inherit"),
    ("messages", "invoker"),
    ("fetch", "held-writes"),
    ("schedules", "switch"),
    ("workspace-actions", "held"),
];

fn modes_of(env: &AppEnvironment, spec: &PlaneSpec) -> Vec<(&'static str, &'static str)> {
    spec.resolve(env)
        .expect("a non-production environment has planes")
        .entries()
        .into_iter()
        .map(|(plane, mode)| (plane.key(), mode))
        .collect()
}

#[test]
fn empty_spec_resolves_to_todays_modes() {
    let spec = PlaneSpec::default();
    assert_eq!(modes_of(&AppEnvironment::Staging, &spec), TODAYS_MODES);

    assert_eq!(modes_of(&dev(), &spec), TODAYS_MODES);

    let refused = PlaneSpec::parse(&json!({"workspace-actions": "refused"})).expect("a valid spec");
    let mut sandbox = TODAYS_MODES;
    sandbox[11] = ("workspace-actions", "refused");
    assert_eq!(
        modes_of(&dev(), &refused),
        sandbox,
        "refused stays a choice"
    );
}

#[test]
fn production_has_no_planes() {
    assert_eq!(
        PlaneSpec::default().resolve(&AppEnvironment::Production),
        None
    );
}

#[test]
fn unknown_plane_is_refused() {
    let error = PlaneSpec::parse(&json!({"olpt": "schema"})).expect_err("a typo'd plane");
    assert!(error.contains("olpt"), "{error}");
}

#[test]
fn unknown_mode_is_refused() {
    PlaneSpec::parse(&json!({"oltp": "bogus"})).expect_err("a mode no plane has");
    PlaneSpec::parse(&json!({"oltp": 3})).expect_err("a mode that is not a string");
}

#[test]
fn the_oltp_plane_takes_a_branch_or_an_empty_one() {
    for (wire, mode) in [("branch", OltpMode::Branch), ("empty", OltpMode::Empty)] {
        let spec = PlaneSpec::parse(&json!({ "oltp": wire })).expect("a valid spec");
        assert_eq!(spec.oltp, Some(mode));
        assert_eq!(mode.as_str(), wire);
        assert_eq!(modes_of(&dev(), &spec)[3], ("oltp", wire));
    }
}

#[test]
fn explicit_mode_overrides_default() {
    let spec = PlaneSpec::parse(&json!({"workspace-actions": "refused"})).expect("a valid spec");
    for environment in [dev(), AppEnvironment::Staging] {
        let modes = modes_of(&environment, &spec);
        assert_eq!(modes[11], ("workspace-actions", "refused"));
        assert_eq!(modes[..11], TODAYS_MODES[..11]);
    }
}

#[test]
fn an_empty_object_parses_to_the_default_spec() {
    assert_eq!(PlaneSpec::parse(&json!({})), Ok(PlaneSpec::default()));
}

#[test]
fn default_parent_per_kind() {
    assert_eq!(
        default_parent(&AppEnvironment::Staging),
        Some(AppEnvironment::Production)
    );
    assert_eq!(default_parent(&dev()), Some(AppEnvironment::Staging));
    assert_eq!(default_parent(&AppEnvironment::Production), None);
}

/// The keys are what `env show` and a `planes` column spell: a wire contract.
#[test]
fn plane_keys_are_the_wire_contract() {
    let keys: Vec<&str> = Plane::ALL.iter().map(|p| p.key()).collect();
    assert_eq!(
        keys,
        [
            "build",
            "semantic",
            "reads",
            "oltp",
            "airhouse",
            "warehouse",
            "storage",
            "secrets",
            "messages",
            "fetch",
            "schedules",
            "workspace-actions",
        ]
    );
}

/// `Plane::ALL` lists every variant, in declaration order: the wildcard-free
/// `match` stops compiling when a variant is added, and its arm count is
/// then the number `ALL` must reach.
#[test]
fn plane_all_is_exhaustive() {
    fn index(plane: Plane) -> usize {
        match plane {
            Plane::Build => 0,
            Plane::Semantic => 1,
            Plane::Reads => 2,
            Plane::Oltp => 3,
            Plane::Airhouse => 4,
            Plane::Warehouse => 5,
            Plane::Storage => 6,
            Plane::Secrets => 7,
            Plane::Messages => 8,
            Plane::Fetch => 9,
            Plane::Schedules => 10,
            Plane::WorkspaceActions => 11,
        }
    }
    const ARMS: usize = 12;
    assert_eq!(Plane::ALL.len(), ARMS);
    for (position, plane) in Plane::ALL.iter().enumerate() {
        assert_eq!(index(*plane), position, "{plane:?} is out of order in ALL");
    }
}

/// Every `PlaneSpec` field is named by `Plane::key`, and every mode's
/// `as_str` is its serde name: spelling `{key: mode}` back through `parse`
/// resolves to the same entry. `deny_unknown_fields` refuses a field whose
/// name drifted from its key, and a mode whose `as_str` drifted from its serde
/// name fails to parse.
#[test]
fn every_plane_and_mode_spelling_round_trips_through_parse() {
    for env in [AppEnvironment::Staging, dev()] {
        let defaults = PlaneSpec::default()
            .resolve(&env)
            .expect("a non-production environment has planes");
        for (plane, mode) in defaults.entries() {
            let spec = PlaneSpec::parse(&json!({ plane.key(): mode }))
                .unwrap_or_else(|e| panic!("{} = {mode} on {env:?}: {e}", plane.key()));
            assert_ne!(
                spec,
                PlaneSpec::default(),
                "{} left the spec unset",
                plane.key()
            );
            let entry = spec
                .resolve(&env)
                .expect("a non-production environment has planes")
                .entries()
                .into_iter()
                .find(|(p, _)| *p == plane);
            assert_eq!(entry, Some((plane, mode)), "{} on {env:?}", plane.key());
        }
    }
}

fn staging_planes(spec: &PlaneSpec) -> super::ResolvedPlanes {
    spec.resolve(&AppEnvironment::Staging)
        .expect("a non-production environment has planes")
}

#[test]
fn parses_semantic_branch() {
    let spec = PlaneSpec::parse(&json!({"semantic": "branch:feat-x"})).expect("a valid spec");
    assert_eq!(spec.semantic_branch(), Some("feat-x"));
    assert_eq!(staging_planes(&spec).semantic, SemanticMode::Branch);
    // A branch name may carry `/`, as git's own do.
    let nested = PlaneSpec::parse(&json!({"semantic": "branch:feat/x"})).expect("a valid spec");
    assert_eq!(nested.semantic_branch(), Some("feat/x"));
    // Build-pin names no branch.
    let pin = PlaneSpec::parse(&json!({"semantic": "build-pin"})).expect("a valid spec");
    assert_eq!(pin.semantic_branch(), None);
    assert_eq!(staging_planes(&pin).semantic, SemanticMode::BuildPin);
    assert_eq!(PlaneSpec::default().semantic_branch(), None);
}

#[test]
fn refuses_bare_branch() {
    for value in ["branch", "branch:"] {
        let error = PlaneSpec::parse(&json!({"semantic": value})).expect_err(value);
        assert!(error.contains("semantic"), "{value}: {error}");
    }
    // `release:staging` waits on W4's data releases.
    let error = PlaneSpec::parse(&json!({"semantic": "release:staging"})).expect_err("not yet");
    assert!(error.contains("semantic"), "{error}");
}

#[test]
fn refuses_bad_branch_name() {
    for name in [
        "a..b",
        "has space",
        "-lead",
        "/lead",
        "trail/",
        "x.lock",
        "tab\there",
    ] {
        let value = format!("branch:{name}");
        let error = PlaneSpec::parse(&json!({"semantic": value})).expect_err(&value);
        assert!(error.contains("semantic"), "{value}: {error}");
    }
    let long = "a".repeat(201);
    valid_branch_name(&long).expect_err("over 200 bytes");
    valid_branch_name(&"a".repeat(200)).expect("exactly 200 bytes");
    valid_branch_name("").expect_err("empty");
}

#[test]
fn parses_workspace_actions_preview() {
    let spec = PlaneSpec::parse(&json!({"workspace-actions": "preview"})).expect("a valid spec");
    for env in [AppEnvironment::Staging, dev()] {
        let modes = spec.resolve(&env).expect("planes").workspace_actions;
        assert_eq!(modes, WorkspaceActionsMode::Preview, "{env:?}");
    }
    let dev_modes = modes_of(&dev(), &spec);
    assert_eq!(
        dev_modes
            .iter()
            .find(|(plane, _)| *plane == "workspace-actions"),
        Some(&("workspace-actions", "preview"))
    );
}

#[test]
fn round_trips_semantic_choice() {
    for choice in [
        SemanticChoice::BuildPin,
        SemanticChoice::Branch("feat/x".to_string()),
    ] {
        let value = serde_json::to_value(&choice).expect("serializes");
        assert!(value.is_string(), "{value}");
        let back: SemanticChoice = serde_json::from_value(value).expect("parses back");
        assert_eq!(back, choice);
    }
    assert_eq!(
        serde_json::to_value(SemanticChoice::Branch("b".into())).expect("serializes"),
        json!("branch:b")
    );
}

/// Each rule of git's `check-ref-format` that `valid_branch_name` enforces, by
/// the name that breaks it; and names git accepts pass.
#[test]
fn valid_branch_name_follows_git_check_ref_format() {
    let bad = [
        ("", "empty"),
        ("a b", "space"),
        ("a\tb", "tab"),
        ("a\nb", "newline"),
        ("a\u{7f}b", "DEL"),
        ("a\u{1}b", "control"),
        ("a~b", "tilde"),
        ("a^b", "caret"),
        ("a:b", "colon"),
        ("a?b", "question mark"),
        ("a*b", "asterisk"),
        ("a[b", "open bracket"),
        ("a\\b", "backslash"),
        ("a@{b", "reflog syntax"),
        ("a//b", "double slash"),
        ("/a", "leading slash"),
        ("-a", "leading dash"),
        ("a/", "trailing slash"),
        ("a.", "trailing dot"),
        (".a", "leading dot"),
        ("a/.b", "component starting with a dot"),
        ("a.lock", "ends with .lock"),
        ("a.lock/b", "component ending with .lock"),
        ("a..b", "double dot"),
        ("@", "the bare at sign"),
    ];
    for (name, why) in bad {
        assert!(valid_branch_name(name).is_err(), "{name:?} ({why}) passed");
    }
    for good in [
        "main",
        "feat-x",
        "feat/x",
        "release/1.2.3",
        "user@example",
        "a.b",
        "@feat",
        "x_y-z",
        "ünïcode",
    ] {
        assert_eq!(valid_branch_name(good), Ok(()), "{good:?}");
    }
}

/// The object form `{"mode": "branch", "branch": "<b>"}` (P1's settings shape)
/// reads as the same choice as `"branch:<b>"`; serializing writes the string.
#[test]
fn semantic_choice_reads_the_object_form_and_writes_the_string() {
    let from_object = |value| serde_json::from_value::<SemanticChoice>(value);
    let branch = SemanticChoice::Branch("feat/x".to_string());
    assert_eq!(
        from_object(json!({"mode": "branch", "branch": "feat/x"})).expect("object form"),
        branch
    );
    assert_eq!(
        from_object(json!("branch:feat/x")).expect("string form"),
        branch
    );
    assert_eq!(
        from_object(json!({"mode": "build-pin"})).expect("object build-pin"),
        SemanticChoice::BuildPin
    );
    assert_eq!(
        serde_json::to_value(&branch).expect("serializes"),
        json!("branch:feat/x")
    );
    for bad in [
        json!({"mode": "branch"}),
        json!({"mode": "branch", "branch": ""}),
        json!({"mode": "branch", "branch": "a..b"}),
        json!({"mode": "branch", "branch": 3}),
        json!({"mode": "build-pin", "branch": "x"}),
        json!({"mode": "release"}),
        json!({"branch": "x"}),
        json!({"mode": "branch", "branch": "x", "extra": 1}),
        json!(3),
        json!(null),
    ] {
        let error = from_object(bad.clone()).expect_err(&bad.to_string());
        assert!(error.to_string().contains("semantic"), "{bad}: {error}");
    }
    // Through a whole spec: both forms resolve to the same branch.
    let spec = PlaneSpec::parse(&json!({"semantic": {"mode": "branch", "branch": "feat-x"}}))
        .expect("object form in a spec");
    assert_eq!(spec.semantic_branch(), Some("feat-x"));
}

/// P3: the Airhouse plane's overlay modes parse and resolve; nothing else on
/// the environment moves.
#[test]
fn airhouse_overlay_modes_parse() {
    for mode in ["branch", "snapshot"] {
        let spec = PlaneSpec::parse(&json!({ "airhouse": mode })).expect(mode);
        let mut expected = TODAYS_MODES;
        expected[4] = ("airhouse", mode);
        assert_eq!(modes_of(&AppEnvironment::Staging, &spec), expected);
    }
}

/// A preset never opens third-party writes: they need a person's host list.
#[test]
fn no_preset_entry_names_the_fetch_plane() {
    assert!(
        super::PRESET_FULL
            .iter()
            .all(|(plane, _)| *plane != Plane::Fetch)
    );
}
