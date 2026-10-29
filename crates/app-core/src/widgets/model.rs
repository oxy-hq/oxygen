//! A widget as the manifest declares it.

use serde::Serialize;

/// At most this many widgets per app: the home is a glance, not a dashboard.
pub const MAX_WIDGETS: usize = 3;
/// At most this many actions per widget.
pub const MAX_ACTIONS: usize = 3;
pub const MAX_TITLE_CHARS: usize = 40;

/// How wide the home draws the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    #[default]
    Regular,
    /// Two columns.
    Wide,
}

impl Size {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "regular" => Some(Self::Regular),
            "wide" => Some(Self::Wide),
            _ => None,
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
    pub size: Size,
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
