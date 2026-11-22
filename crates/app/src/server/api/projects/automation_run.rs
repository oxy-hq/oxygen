//! The custom-app automation run's **row**, re-exported at its old paths. Its
//! state machine (`settle`), the failure classifier and the preview-ownership
//! read went down to `oxy_custom_apps::automation_run` on 2026-11-22.
//!
//! [`preview_settle`], settling a preview-mode run's row from its finished
//! preview run (which the workspace-preview sweep drives in the same tick it
//! marks the preview run finished), went down with the previews rows the same
//! day: it is `oxy_previews::queue::preview_settle`, beside the sweep.

pub use oxy_custom_apps::automation_run::{automation_error_to_code, owns_run, settle};

pub use oxy_previews::queue::preview_settle;
