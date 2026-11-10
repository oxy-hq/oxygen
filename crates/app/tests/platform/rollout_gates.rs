//! The rollout gates (`feature_flags::rollout`) decide from the flag table once
//! their environment override is unset — and every process that runs a gated
//! path loads that table at boot.
//!
//! The unit tests in `rollout.rs` inject the lookups, so they pin the
//! precedence but never that the real `is_on` sees a real flag row. The hole
//! this guards: `oxy worker` ran the worker-fleet compile, the reconcile tick
//! and `self_heal` without ever loading the flag cache, so with the variable
//! unset every gate read its registry default — off — while the admin page
//! said it was on. `oxy compile` had the same gap.

use oxy_app::server::compile_config_gate::runtime_config_gate;
use oxy_app::server::compile_reconcile;
use oxy_app::server::feature_flags::rollout::{
    COMPILE_RECONCILE, COMPILE_REFUSE_CLASSIC_AGENTS_FLAG, COMPILE_REFUSE_FILE_CREDENTIALS,
    COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS, is_on,
};
use oxy_app::server::feature_flags::{cache, store};

use crate::common::test_db;

/// A flag row in the database — as `oxyc flags set <flag> --on` leaves it —
/// turns the gate on for a process that loaded the cache the way `oxy worker`
/// does, with none of the variables set.
#[tokio::test]
async fn with_the_variables_unset_a_flag_row_turns_the_gate_on() {
    // A migrated per-test DB; `OXY_DATABASE_URL` points at it, which is what
    // `cache::init` opens.
    let db = test_db().await;
    // SAFETY: nextest runs each test in its own process (`test_db` asserts it),
    // so unsetting the overrides touches nobody else.
    for gate in [
        &COMPILE_RECONCILE,
        &COMPILE_REFUSE_FILE_CREDENTIALS,
        &COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS,
    ] {
        unsafe { std::env::remove_var(gate.env) };
    }

    store::upsert(&db, COMPILE_RECONCILE.flag, true)
        .await
        .expect("flip compile_reconcile on");
    store::upsert(&db, COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS.flag, true)
        .await
        .expect("flip compile_refuse_retired_automation_steps on");
    store::upsert(&db, COMPILE_REFUSE_CLASSIC_AGENTS_FLAG, true)
        .await
        .expect("flip compile_refuse_classic_agents on");

    // Before the load the gates read the registry default — the state a
    // process that never called `init` stays in for its whole life.
    assert!(
        !is_on(&COMPILE_RECONCILE),
        "unloaded: registry default, off"
    );

    cache::init()
        .await
        .expect("feature flag cache init, as the worker does");

    assert!(is_on(&COMPILE_RECONCILE), "the row decides once loaded");
    assert!(
        compile_reconcile::enabled(),
        "the reconcile tick and self_heal read the same gate"
    );
    assert!(
        runtime_config_gate().refuses_retired_automation_steps(),
        "the compile gate the worker installs carries the flag into oxy-compile"
    );
    assert!(
        runtime_config_gate().refuses_classic_agents(),
        "the classic-agent refusal reaches oxy-compile the same way"
    );
    assert!(
        !is_on(&COMPILE_REFUSE_FILE_CREDENTIALS),
        "a gate with no row keeps its default, off"
    );
}

/// Every process that runs a gated path loads the flag cache at boot: `oxy
/// serve` and `oxy worker` through `init_feature_flags` (one load plus the
/// refresh), `oxy compile` through `cache::load_once`. Source-level, because
/// the boot sequences need a database and a signal to run end to end, and the
/// hole was a missing call, not a wrong one.
#[test]
fn every_process_that_compiles_loads_the_flag_cache_at_boot() {
    let commands = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/commands");
    for (file, call) in [
        ("serve.rs", "init_feature_flags().await?"),
        ("worker.rs", "serve::init_feature_flags().await?"),
        ("compile.rs", "feature_flags::cache::load_once().await?"),
    ] {
        let path = commands.join(file);
        let source =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(
            source.contains(call),
            "{file} must load the feature-flag cache at boot (`{call}`): its compile \
             paths read the rollout gates through `feature_flags::rollout::is_on`, and \
             an unloaded cache reads every gate at its registry default, off"
        );
    }
}
