//! Home widgets: a custom app's live cards on the Oxygen home.
//!
//! An app declares widgets in its manifest (`widgets`); each names the
//! function that returns its view and the actions its buttons may run. This
//! module owns the declaration (`model`), its lenient runtime read (`parse`),
//! its strict publish check (`validate`) and the check of the view a widget
//! function returns (`view`). The routes live in `oxy-api-custom-apps`
//! (`home_widgets`). Design: `internal-docs/home-widgets.md`.

pub mod model;
pub mod parse;
pub mod validate;
pub mod view;

#[cfg(test)]
mod validate_tests;
#[cfg(test)]
mod view_tests;
