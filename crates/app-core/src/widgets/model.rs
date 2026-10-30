//! A widget as the manifest declares it.

use serde::Serialize;

/// At most this many widgets per app. The org's home layout picks which show;
/// the home is a glance, not a dashboard.
pub const MAX_WIDGETS: usize = 6;
/// At most this many actions per widget.
pub const MAX_ACTIONS: usize = 3;
pub const MAX_TITLE_CHARS: usize = 40;

/// How wide the home draws the card: a named size, never a coordinate. The
/// home packs cards onto its grid in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    /// One column.
    #[default]
    Regular,
    /// Two columns.
    Wide,
    /// The whole row.
    Full,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Regular, Size::Wide, Size::Full];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "regular" => Some(Self::Regular),
            "wide" => Some(Self::Wide),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Wide => "wide",
            Self::Full => "full",
        }
    }
}

/// One declared widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetDef {
    pub id: String,
    pub title: String,
    /// The function that returns the widget's view.
    pub function: String,
    /// The size it is drawn at unless a layout picks another.
    pub size: Size,
    /// The sizes a layout may pick; always includes `size`.
    pub sizes: Vec<Size>,
    /// The settings a layout gives each copy, when it declares any.
    pub params: Option<super::params::ParamsSchema>,
    pub actions: Vec<ActionDef>,
}

impl WidgetDef {
    pub fn action(&self, id: &str) -> Option<&ActionDef> {
        self.actions.iter().find(|a| a.id == id)
    }
}

/// One action a widget's buttons may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionDef {
    pub id: String,
    pub function: String,
    /// The home asks before running it.
    pub confirm: bool,
    /// Drawn as a destructive button.
    pub destructive: bool,
}
