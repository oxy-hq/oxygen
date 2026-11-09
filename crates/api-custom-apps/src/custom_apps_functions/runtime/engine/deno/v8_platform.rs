//! V8's process-wide setup: the flags every isolate runs under, then the
//! platform.
//!
//! V8 flags are process-wide and frozen once V8 initializes, so they must be
//! set before the first isolate exists. Every path in this crate that creates
//! one comes through [`init`] first — the snapshot builder
//! (`snapshot::build`) and `DenoEngine::create_with`, which every engine,
//! warm-pool slot and test isolate is made by — so there is no isolate that
//! runs without them.

use std::sync::OnceLock;

use deno_core::JsRuntime;

/// The flags V8 is started with, for every isolate in the process.
///
/// `--no-turboshaft-loop-unrolling` closes a **known V8 gap**: when Turboshaft
/// unrolls an allocating loop such as
/// `for (;;) keep.push(new Array(64).fill(i))`, the optimised code loses its
/// interrupt check, so neither the broker's `terminate_execution` nor the
/// near-heap-limit callback ever lands. Without the flag the isolate thread is
/// abandoned spinning a core after `Timeout`, and with unbounded memory V8 then
/// aborts the whole process on a fatal OOM — one tenant's function takes down
/// `oxy serve`. Present on V8 15.0 (deno_core 0.412/0.413) and 15.2.
///
/// Measured cost: none on typical or numeric functions. `--no-turbofan` closes
/// the gap too, at up to +46% on numeric code. Pinned by
/// `runtime/terminate_gap_tests.rs`: drop the flag only once those pass
/// without it on a newer V8.
pub(super) const V8_FLAGS: &[&str] = &["--no-turboshaft-loop-unrolling"];

/// Set when V8 refuses a flag in [`V8_FLAGS`], so the gap stays closed (at up
/// to +46% on numeric code) rather than reopening silently. Refusing to start
/// isolates instead would take down every function.
const FALLBACK_FLAGS: &[&str] = &["--no-turbofan"];

/// Set [`V8_FLAGS`] and initialize V8 for execution, once per process.
///
/// Returns the flags V8 did not recognise, which is empty unless a V8 bump
/// renamed or removed one; then [`FALLBACK_FLAGS`] are set instead and the
/// refusal is logged as an error. Initializing here also keeps V8 out of
/// snapshot mode (see `snapshot`), whichever path touches it first.
pub(super) fn init() -> &'static [String] {
    static UNRECOGNISED: OnceLock<Vec<String>> = OnceLock::new();
    UNRECOGNISED.get_or_init(|| {
        let unrecognised = set_flags(V8_FLAGS);
        if !unrecognised.is_empty() {
            let fallback_unrecognised = set_flags(FALLBACK_FLAGS);
            tracing::error!(
                target: "oxy.custom_app.v8",
                ?unrecognised,
                fallback = ?FALLBACK_FLAGS,
                ?fallback_unrecognised,
                "V8 did not recognise the function runtime's flags; falling back"
            );
        }
        JsRuntime::init_platform(None);
        unrecognised
    })
}

/// Hand `flags` to V8; returns the ones it did not consume.
fn set_flags(flags: &[&str]) -> Vec<String> {
    let mut args = vec!["oxy".to_string()];
    args.extend(flags.iter().map(|f| (*f).to_string()));
    // V8 hands back what it did not consume, argv[0] first.
    deno_core::v8_set_flags(args).into_iter().skip(1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The terminate-gap flag is in the set, and V8 took it: a V8 that renamed
    /// or dropped it would hand it back unrecognised.
    #[test]
    fn the_terminate_gap_flag_is_set_in_the_process() {
        assert!(
            V8_FLAGS.contains(&"--no-turboshaft-loop-unrolling"),
            "V8_FLAGS no longer closes the terminate gap: {V8_FLAGS:?}"
        );
        assert_eq!(init(), &[] as &[String], "V8 refused runtime flags");
    }
}
