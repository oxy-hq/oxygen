//! The custom-app automation run's **row**. Its state machine (`settle`), the
//! failure classifier and the preview-ownership read went down to
//! `oxy_custom_apps::automation_run` on 2026-11-22 and are re-exported here at
//! their old paths.
//!
//! What stays is [`preview_settle`]: settling a preview-mode run's row from its
//! finished preview run, which the workspace-preview sweep
//! (`previews::queue::sweep`) drives in the same tick it marks the preview run
//! finished. It reads `previews::queue::lifecycle`, so it moves with the
//! previews rows (`oxy-previews`), not with the state machine.

pub use oxy_custom_apps::automation_run::{automation_error_to_code, owns_run, settle};

pub mod preview_settle;
