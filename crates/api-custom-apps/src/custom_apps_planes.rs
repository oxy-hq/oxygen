//! The planes of a custom app live in `oxy-custom-apps` (`planes`), because
//! `oxy-app` reads them too (the agent ask and the automation run on a
//! non-production host) and cannot depend on this crate. Re-exported so every
//! `crate::custom_apps_planes::…` path keeps resolving.

pub use oxy_custom_apps::planes::*;
