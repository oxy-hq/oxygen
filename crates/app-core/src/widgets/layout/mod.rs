//! The org's home layout: Today's widget groups in order, which widgets each holds,
//! in what order, at what size, and whether insights show. The apps always come
//! first; the `apps` section places nothing (`internal-docs/home-widgets.md`).
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
//!
//! A widgets group and an item may carry `for` (`audience`): the people and org
//! teams a copy is for. It is stored and written back by this model, and applied
//! with `visible` when one person's home is drawn.

mod against;
mod audience;
mod check;
mod narrow;
mod resolve;
mod stored;

pub use audience::{Audience, MAX_ENTRIES as MAX_AUDIENCE_ENTRIES, Viewer};
pub use check::{check, read};
pub use narrow::{visible, without};
pub use resolve::{Drawn, Placed, effective, resolve};
pub use stored::{join as join_stored, split as split_for_storage};

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
        /// `for`: who the whole group is for. Absent: everyone.
        audience: Option<Audience>,
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
    /// `for`: who this copy is for, on top of its group's. Absent: the group's.
    pub audience: Option<Audience>,
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
    /// A field this server does not know. A write refuses it; a stored layout is
    /// read past it, so a field a newer server added never costs an older one
    /// (a revert, or old pods mid-rollout) the whole layout.
    #[serde(skip)]
    pub(super) unknown_field: bool,
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
        unknown_field: false,
    }
}
