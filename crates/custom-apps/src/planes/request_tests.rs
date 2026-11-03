use chrono::{Duration, TimeZone, Utc};
use oxy_app_core::custom_app_environment::AppEnvironment;
use serde_json::json;

use super::super::{AirhouseMode, MessagesMode, SecretsMode};
use super::{Plane, PlaneContext, PlaneError, PlaneSpec, expand, validate_fetch};

/// A write to `environment` in an org with a database and room for branches.
fn context(environment: AppEnvironment) -> PlaneContext {
    PlaneContext {
        environment,
        existing_semantic_branch: None,
        fork_at: None,
        requested_at: None,
        now: Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap(),
        org_has_oltp: true,
        other_oltp_branches: 0,
        max_oltp_branches: 8,
        warehouse_refusal: None,
    }
}

fn sandbox() -> AppEnvironment {
    AppEnvironment::parse("dev-a1").expect("a sandbox")
}

fn oltp(mode: &str) -> PlaneSpec {
    PlaneSpec::parse(&json!({ "oltp": mode })).expect("a real mode")
}

fn stored(entries: &[(Plane, &str)], planes: Option<serde_json::Value>) -> serde_json::Value {
    expand(entries, planes.as_ref())
        .expect("a valid request")
        .stored
}

#[test]
fn no_preset_and_no_planes_store_the_empty_object() {
    let requested = PlaneSpec::from_request(None, None).expect("nothing asked");
    assert_eq!(requested.stored, json!({}));
    assert_eq!(requested.spec, PlaneSpec::default());
    let null = PlaneSpec::from_request(None, Some(&json!(null))).expect("null is nothing");
    assert_eq!(null.stored, json!({}));
}

/// `light` is every default; `full` is `PRESET_FULL`, one entry for each
/// plane whose isolating mode has shipped (today OLTP's `branch`, the secrets
/// plane's `empty`, the messages plane's `capture`, Airhouse's `branch` and
/// workspace actions' `preview`; storage stays `branch`). There is no
/// `semantic` entry, which would need a branch name to read.
#[test]
fn both_presets_expand_to_what_ships() {
    let light = PlaneSpec::from_request(Some("light"), None).expect("light");
    assert_eq!(light.stored, json!({}));
    let full = PlaneSpec::from_request(Some("full"), None).expect("full");
    assert_eq!(
        full.stored,
        json!({
            "oltp": "branch",
            "secrets": "empty",
            "messages": "capture",
            "airhouse": "branch",
            "workspace-actions": "preview"
        })
    );
    assert_eq!(full.spec.secrets, Some(SecretsMode::Empty));
    // An explicit key beats the preset's entry.
    let over =
        PlaneSpec::from_request(Some("full"), Some(&json!({"oltp": "schema"}))).expect("override");
    assert_eq!(
        over.stored,
        json!({
            "oltp": "schema",
            "secrets": "empty",
            "messages": "capture",
            "airhouse": "branch",
            "workspace-actions": "preview"
        })
    );
    assert_eq!(full.spec.messages, Some(MessagesMode::Capture));
    assert_eq!(full.spec.airhouse, Some(AirhouseMode::Branch));
    assert_eq!(full.spec.storage, None, "storage keeps its default");
    assert_eq!(full.spec.semantic_branch(), None);
    // An explicit key still wins over the preset.
    let held = PlaneSpec::from_request(Some("full"), Some(&json!({"workspace-actions": "held"})))
        .expect("full with an override");
    assert_eq!(
        held.stored,
        json!({
            "oltp": "branch",
            "secrets": "empty",
            "messages": "capture",
            "airhouse": "branch",
            "workspace-actions": "held"
        })
    );
}

/// `full` asks for OLTP's and Airhouse's branches, which are for sandboxes
/// only: a sandbox may store it, staging is refused with the reason.
#[test]
fn the_full_preset_validates_on_a_sandbox_only() {
    let full = PlaneSpec::from_request(Some("full"), None)
        .expect("full")
        .spec;
    full.validate(&context(sandbox())).expect("a sandbox");
    let refused = full
        .validate(&context(AppEnvironment::Staging))
        .expect_err("staging");
    assert!(refused.0.contains("sandboxes"), "{refused}");
}

#[test]
fn an_unknown_preset_is_refused_by_name() {
    let error = PlaneSpec::from_request(Some("heavy"), None).expect_err("not a preset");
    assert!(error.0.contains("heavy"), "{error}");
}

#[test]
fn planes_that_are_not_an_object_are_refused() {
    for bad in [json!("storage"), json!(["storage"]), json!(3)] {
        PlaneSpec::from_request(None, Some(&bad)).expect_err("planes must be an object");
    }
}

#[test]
fn the_requested_object_is_stored_as_sent() {
    let sent = json!({"workspace-actions": "refused"});
    let requested = PlaneSpec::from_request(None, Some(&sent)).expect("a valid request");
    assert_eq!(requested.stored, sent);
    assert_eq!(
        requested.spec,
        PlaneSpec::parse(&sent).expect("the same object parses")
    );
}

/// A preset's entries come first, and an explicit key beats the preset's.
#[test]
fn explicit_keys_override_the_presets_entries() {
    let preset = [(Plane::Oltp, "schema"), (Plane::WorkspaceActions, "held")];
    assert_eq!(
        stored(&preset, None),
        json!({"oltp": "schema", "workspace-actions": "held"})
    );
    assert_eq!(
        stored(&preset, Some(json!({"workspace-actions": "refused"}))),
        json!({"oltp": "schema", "workspace-actions": "refused"})
    );
}

#[test]
fn an_unknown_plane_or_mode_is_refused_by_name() {
    let plane = PlaneSpec::from_request(None, Some(&json!({"olpt": "schema"})))
        .expect_err("a typo'd plane");
    assert!(plane.0.contains("olpt"), "{plane}");
    let mode = PlaneSpec::from_request(None, Some(&json!({"oltp": "teleport"})))
        .expect_err("a mode that does not exist");
    assert!(mode.0.contains("teleport"), "{mode}");
    // A bad preset entry is refused the same way: it is parsed after expansion.
    expand(&[(Plane::Oltp, "nonsense")], None).expect_err("a preset names a real mode");
}

/// Every mode of every plane, spelled as `entries` does, passes `validate` on
/// each kind of environment. A phase that adds a check adds its refusal case
/// beside this.
#[test]
fn every_default_mode_validates_on_every_kind() {
    for environment in [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a sandbox"),
    ] {
        let ctx = context(environment.clone());
        PlaneSpec::default().validate(&ctx).expect("the defaults");
        let defaults = PlaneSpec::default()
            .resolve(&environment)
            .expect("a non-production environment has planes");
        for (plane, mode) in defaults.entries() {
            let spec = PlaneSpec::parse(&json!({ plane.key(): mode })).expect("a real mode");
            assert_eq!(spec.validate(&ctx), Ok(()), "{} = {mode}", plane.key());
        }
    }
}

#[test]
fn a_sandbox_may_have_an_oltp_branch_of_either_mode() {
    for mode in ["schema", "branch", "empty"] {
        assert_eq!(oltp(mode).validate(&context(sandbox())), Ok(()), "{mode}");
    }
}

#[test]
fn staging_refuses_oltp_branch() {
    for mode in ["branch", "empty"] {
        let error = oltp(mode)
            .validate(&context(AppEnvironment::Staging))
            .expect_err(mode);
        assert_eq!(
            error.0,
            "staging uses the org's OLTP staging branch; oltp modes branch and empty are for \
             sandboxes"
        );
    }
    assert_eq!(
        oltp("schema").validate(&context(AppEnvironment::Staging)),
        Ok(())
    );
}

#[test]
fn an_org_with_no_oltp_database_refuses_a_branch_but_not_schema() {
    let mut ctx = context(sandbox());
    ctx.org_has_oltp = false;
    for mode in ["branch", "empty"] {
        let error = oltp(mode).validate(&ctx).expect_err(mode);
        assert!(error.0.contains("no OLTP database"), "{error}");
    }
    assert_eq!(oltp("schema").validate(&ctx), Ok(()));
}

/// The cap is checked against the org's *other* branches, so an environment
/// that already holds one is not refused at the limit; at the limit with
/// somebody else's, a new one is, and the message names the way out.
#[test]
fn the_cap_refuses_at_the_limit_and_names_schema_mode() {
    let mut ctx = context(sandbox());
    ctx.max_oltp_branches = 3;
    ctx.other_oltp_branches = 2;
    assert_eq!(oltp("branch").validate(&ctx), Ok(()), "one below the limit");
    ctx.other_oltp_branches = 3;
    for mode in ["branch", "empty"] {
        let error = oltp(mode).validate(&ctx).expect_err(mode);
        assert_eq!(
            error.0,
            "this org has 3 sandbox OLTP branches, the most OXY_APP_ENV_OLTP_BRANCHES_MAX \
             allows (3); use --plane oltp=schema, the cheap mode, or delete a sandbox"
        );
    }
    assert_eq!(
        oltp("schema").validate(&ctx),
        Ok(()),
        "schema is the cheap mode"
    );
}

#[test]
fn an_at_outside_the_history_window_is_refused() {
    let mut ctx = context(sandbox());
    let now = ctx.now;
    // Only the write's own `at` is checked: an old stored fork point is not.
    ctx.fork_at = Some(now - Duration::hours(2 * 24));
    assert_eq!(oltp("branch").validate(&ctx), Ok(()));
    ctx.requested_at = Some(now - Duration::hours(23));
    assert_eq!(oltp("branch").validate(&ctx), Ok(()));
    ctx.requested_at = Some(now - Duration::hours(2 * 24));
    let old = oltp("branch").validate(&ctx).expect_err("too old");
    assert!(old.0.contains("hours of history"), "{old}");
    ctx.requested_at = Some(now + Duration::hours(1));
    let future = oltp("empty").validate(&ctx).expect_err("in the future");
    assert!(future.0.contains("future"), "{future}");
    // A mode with no branch has no fork point to check.
    assert_eq!(oltp("schema").validate(&ctx), Ok(()));
}

/// The same for an Airhouse `snapshot`: only a create's `at` is held to the
/// snapshot window. An existing sandbox's old fork point never refuses saving
/// its planes; its pin is checked where it is read, and the reset re-pins it.
#[test]
fn only_a_requested_at_is_held_to_the_snapshot_window() {
    let snapshot = PlaneSpec::parse(&json!({ "airhouse": "snapshot" })).expect("a real mode");
    let mut ctx = context(sandbox());
    let now = ctx.now;
    ctx.fork_at = Some(now - Duration::hours(2 * 24));
    assert_eq!(snapshot.validate(&ctx), Ok(()));
    ctx.requested_at = Some(now - Duration::hours(2 * 24));
    let old = snapshot.validate(&ctx).expect_err("too old");
    assert!(old.0.contains("--at within the last"), "{old}");
}

#[test]
fn a_plane_error_displays_its_message() {
    assert_eq!(PlaneError("bad".into()).to_string(), "bad");
}

/// A `semantic: branch:<b>` choice validates only against a context that
/// records `<b>` as existing: a route that skipped the lookup, or looked up a
/// different branch, is refused with a message naming the branch.
#[test]
fn a_semantic_branch_validates_only_against_a_checked_one() {
    let spec = PlaneSpec::parse(&json!({"semantic": "branch:feat-x"})).expect("a valid spec");
    let ctx = context(AppEnvironment::Staging);
    let unchecked = spec.validate(&ctx).expect_err("the branch was not checked");
    assert!(unchecked.0.contains("feat-x"), "{unchecked}");
    let other = spec
        .validate(&ctx.clone().with_existing_branch("main"))
        .expect_err("a different branch was checked");
    assert!(other.0.contains("feat-x"), "{other}");
    assert_eq!(
        spec.validate(&ctx.clone().with_existing_branch("feat-x")),
        Ok(())
    );
    // build-pin and no choice name no branch, so there is nothing to check.
    for names_none in [json!({"semantic": "build-pin"}), json!({})] {
        let spec = PlaneSpec::parse(&names_none).expect("a valid spec");
        assert_eq!(spec.validate(&ctx), Ok(()), "{names_none}");
    }
}

/// `fetch` is the one plane `validate` checks today: a setting
/// `validate_fetch` refuses is refused with its message, on every kind.
#[test]
fn validate_refuses_a_fetch_setting_validate_fetch_refuses() {
    for environment in [
        AppEnvironment::Staging,
        AppEnvironment::parse("dev-a1").expect("a sandbox"),
    ] {
        let ctx = context(environment);
        let bad = PlaneSpec::parse(&json!({"fetch": {"mode": "allow", "hosts": ["example.com"]}}))
            .expect("it parses");
        let error = bad.validate(&ctx).expect_err("not a registered sandbox");
        let expected = validate_fetch(bad.fetch.as_ref().expect("fetch is set"))
            .expect_err("validate_fetch refuses it");
        assert_eq!(error, PlaneError(expected));
        assert!(error.0.contains("example.com"), "{error}");
        let no_hosts = PlaneSpec::parse(&json!({"fetch": "allow"})).expect("it parses");
        assert_eq!(
            no_hosts.validate(&ctx),
            Err(PlaneError("fetch: allow needs at least one host".into()))
        );
        let good = PlaneSpec::parse(
            &json!({"fetch": {"mode": "allow", "hosts": ["connect.squareupsandbox.com"]}}),
        )
        .expect("it parses");
        assert_eq!(good.validate(&ctx), Ok(()));
        let held = PlaneSpec::parse(&json!({"fetch": "held-writes"})).expect("it parses");
        assert_eq!(held.validate(&ctx), Ok(()));
    }
}

/// The write path runs `from_request` and then `validate`, so a bad fetch
/// setting parses (it is well-formed) and is only refused by `validate`.
#[test]
fn from_request_accepts_what_only_validate_refuses() {
    let sent = json!({"fetch": {"mode": "allow", "hosts": ["example.com"]}});
    let requested = PlaneSpec::from_request(None, Some(&sent)).expect("it parses");
    let ctx = context(AppEnvironment::Staging);
    requested.spec.validate(&ctx).expect_err("validate refuses");
}
