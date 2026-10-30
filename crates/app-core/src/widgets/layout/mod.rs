//! The org's home layout: which sections the home draws, in what order, and in
//! the widgets section which widgets, in what order, at what size.
//!
//! Placement is by order and named size, packed onto the grid by the home —
//! never by coordinates — so an LLM or `oxyc` writing a layout has no geometry
//! to get wrong. Content (each app's widgets, declared in its manifest) and
//! placement (this document) stay apart, the way Grafana's dashboard schema v2
//! keeps `elements` apart from `layout`. Design: `internal-docs/home-widgets.md`.
//!
//! `check` is the strict read for a write: every problem, each with a path and a
//! fix, so an agent corrects them all in one pass. `read` is the lenient read of
//! a stored layout: a widget an app has since dropped is skipped, a size it no
//! longer allows falls back to its default.

mod against;
mod check;
mod resolve;

pub use check::{check, read};
pub use resolve::{Drawn, Placed, effective, resolve};

use serde::Serialize;
use serde_json::{Map, Value};

use super::model::{Size, WidgetDef};

pub const LAYOUT_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionKind {
    /// The app widgets.
    Widgets,
    /// "Your apps".
    Apps,
    /// The world model and recent questions.
    Insights,
}

impl SectionKind {
    pub const DEFAULT_ORDER: [SectionKind; 3] = [
        SectionKind::Widgets,
        SectionKind::Apps,
        SectionKind::Insights,
    ];

    pub(super) fn parse(s: &str) -> Option<Self> {
        match s {
            "widgets" => Some(Self::Widgets),
            "apps" => Some(Self::Apps),
            "insights" => Some(Self::Insights),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Widgets => "widgets",
            Self::Apps => "apps",
            Self::Insights => "insights",
        }
    }
}

/// One widget the org's apps declare, as a layout names it: `<app>/<widget>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub key: String,
    pub def: WidgetDef,
}

impl CatalogEntry {
    pub fn new(app_slug: &str, def: WidgetDef) -> Self {
        Self {
            key: format!("{app_slug}/{}", def.id),
            def,
        }
    }
}

/// A layout, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    /// A group of widgets. `widgets` sections may repeat; `hide_others` is read
    /// on the first one only, where the widgets a layout does not list go.
    Widgets {
        title: Option<String>,
        items: Vec<Item>,
        hide_others: bool,
    },
    Apps,
    Insights,
}

/// One copy of a widget in a layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub widget: String,
    /// Tells two copies of one widget apart.
    pub id: Option<String>,
    /// Shown instead of the widget's own title.
    pub title: Option<String>,
    pub size: Option<Size>,
    pub hidden: bool,
    /// This copy's settings, checked against the widget's schema.
    pub params: Option<Map<String, Value>>,
}

impl Item {
    /// How a copy is named: `<app>/<widget>` or `<app>/<widget>#<id>`.
    pub fn key(&self) -> String {
        copy_key(&self.widget, self.id.as_deref())
    }
}

pub fn copy_key(widget: &str, id: Option<&str>) -> String {
    match id {
        Some(id) => format!("{widget}#{id}"),
        None => widget.to_string(),
    }
}

/// Why a layout is refused: where, what, and one fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub path: String,
    pub message: String,
    pub fix: String,
}

pub(super) fn problem(
    path: impl Into<String>,
    message: impl Into<String>,
    fix: impl Into<String>,
) -> Problem {
    Problem {
        path: path.into(),
        message: message.into(),
        fix: fix.into(),
    }
}
