//! Platform glue: database connection pooling + encrypted org secret store.
//!
//! Extracted from `oxy` core so leaf crates (`airhouse`, future integrations)
//! can use these primitives without depending on the full `oxy` crate. The
//! `oxy` crate re-exports these symbols at their original paths for source
//! compatibility — see `oxy::database::client` and `oxy::adapters::secrets`.

pub mod db;
/// The `users` query filters, now `entity::filters`; re-exported at their old
/// path so no caller changed.
pub mod filters {
    pub use entity::filters::*;
}
pub mod secrets;
