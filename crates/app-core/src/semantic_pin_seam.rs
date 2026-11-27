//! The port a planes write calls to pin a custom-app environment's semantic
//! model to a workspace git branch.
//!
//! It lives in the kernel (`oxy_shared::semantic_pin_seam`) so that its
//! implementer, `oxy-compile-runtime`, need not depend on this crate; it is
//! re-exported here at the path every surface already names.

pub use oxy_shared::semantic_pin_seam::{PinRefusal, SemanticPinner};
