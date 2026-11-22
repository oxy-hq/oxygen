//! Sentry classification pins for the preview read-only table, which went
//! down from `oxy-app`'s `server::previews::read_only` into `oxy-previews`
//! in decompose P2 (2026-11-22). Beside `sentry_filter.rs` rather than inside
//! it: that file is over its size budget, and these tests read only its
//! public API.

use tracing::Level;

use crate::sentry_filter::{Disposition, drop_log, is_data_plane_module, sentry_disposition};

/// Kept and not the data plane, at its old path (where the axum adapter
/// stays) and its new one, as the rest of `oxy_previews` is.
#[test]
fn the_preview_read_only_table_keeps_its_sentry_line_after_moving_down() {
    for module in [
        "oxy_app::server::previews::read_only",
        "oxy_previews::read_only",
    ] {
        assert_eq!(
            sentry_disposition(module, &Level::ERROR),
            Disposition::Event,
            "{module}"
        );
        assert!(!drop_log(Some(module)), "{module}");
        assert!(!is_data_plane_module(module), "{module}");
    }
}
