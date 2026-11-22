//! Sentry classification pins for the preview platform and its run resolver,
//! which went down from `oxy-app` into `oxy-workspace-runtime` in decompose W5
//! (2026-11-22). Beside `sentry_filter.rs` rather than inside it: that file is
//! over its size budget, and these tests read only its public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// `agentic_wiring::preview_ctx` (and its submodules) and
/// `server::previews::runtime` were platform code in `oxy-app`: kept, and not
/// the data plane (neither path was ever a `DATA_PLANE_MODULE_PREFIXES`
/// entry). They stay so at their new paths (`preview_ctx`, and the resolver as
/// `preview_runtime`), and the old ones, still re-exported, are pinned beside
/// them. Most of their lines name `target: "preview"`, which the move does not
/// touch; the rest log under the module path pinned here.
#[test]
fn the_preview_platform_keeps_its_sentry_lines_after_moving_down() {
    let mut pairs = vec![
        (
            "oxy_app::agentic_wiring::preview_ctx".to_string(),
            "oxy_workspace_runtime::preview_ctx".to_string(),
        ),
        (
            "oxy_app::server::previews::runtime".to_string(),
            "oxy_workspace_runtime::preview_runtime".to_string(),
        ),
    ];
    for sub in [
        "airhouse",
        "airhouse_batches",
        "airhouse_copies",
        "airhouse_samples",
        "airhouse_writes",
        "names",
        "project",
        "sample",
        "sample_secrets",
        "secrets",
        "workspace",
    ] {
        pairs.push((
            format!("oxy_app::agentic_wiring::preview_ctx::{sub}"),
            format!("oxy_workspace_runtime::preview_ctx::{sub}"),
        ));
    }
    for (old, new) in &pairs {
        for module in [old.as_str(), new.as_str()] {
            assert_eq!(
                sentry_disposition(module, &Level::ERROR),
                Disposition::Event,
                "{module}"
            );
            assert!(!drop_log(Some(module)), "{module}");
            assert!(!is_data_plane_module(module), "{module}");
        }
    }
}
