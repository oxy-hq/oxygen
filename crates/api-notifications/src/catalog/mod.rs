//! The notification catalogue: where every notification sits.
//!
//! ```text
//! source        oxygen | app (one per custom app)
//!  └ category   a path of 0–3 segments: "operations", "operations/food-safety"
//!     └ kind    the leaf: "temp_alarm" — label, level, actions
//! ```
//!
//! The shape iOS categories and thread ids and Android channel groups and
//! channels need. An app declares its tree in `oxy-app.json`'s
//! `notifications` block, checked at publish ([`validate`]) and read from its
//! live build ([`live`]); Oxygen's is in code ([`platform`]). A function's
//! `notify.kinds` still decides which kinds it may send; the catalogue only
//! labels, places and levels them, and gives them actions.
//!
//! Design: `internal-docs/custom-app-notifications.md`.

pub mod audience;
pub mod handler;
pub mod live;
pub mod model;
pub mod parse;
pub mod platform;
pub mod source;
pub mod tree;
pub mod validate;
