//! The catalogue of notification kinds: the model, parsing an app's declaration
//! and validating it, and the port another surface reads an app's live kinds
//! through ([`live_kinds`]). The live catalogue, tree, handler and Oxygen's own
//! catalogue are the notifications surface's (`oxy-api-notifications`).

pub mod live_kinds;
pub mod model;
pub mod parse;
pub mod validate;
