//! An app's notification kinds as its live build describes them, for a surface
//! other than notifications.
//!
//! The staff console's "Test notifications" (`oxy-api-custom-apps`,
//! `admin/apps/test_notification.rs`) lists an app's kinds and sends one. The
//! catalogue it reads is the notifications surface's (`catalog::live` in
//! `oxy-api-notifications`), and one surface never imports another (rule S3),
//! so the reading is a port: `oxy-server` layers the notifications surface's
//! implementation onto the custom-app routes as an `Extension`. A route that
//! finds none answers 500 rather than an empty list — a missing port is a
//! composition error, never "this app declares no kinds".

use async_trait::async_trait;
use entity::apps;
use sea_orm::{DatabaseConnection, DbErr};

use super::model::Level;

/// One kind of an app's live catalogue: what the console shows and what its
/// test send needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindSummary {
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
    pub level: Level,
    /// The labels from the top category down to its own.
    pub category_labels: Vec<String>,
    /// Whether a person's mute stops it: false for a critical kind.
    pub mutable: bool,
}

impl KindSummary {
    pub fn critical(&self) -> bool {
        self.level == Level::Critical
    }
}

/// Reads an app's live catalogue.
#[async_trait]
pub trait LiveKinds: Send + Sync {
    /// Every kind `app`'s live build describes or lets a function send, in
    /// kind order. An app with no live build has none.
    async fn live_kinds(
        &self,
        db: &DatabaseConnection,
        app: &apps::Model,
    ) -> Result<Vec<KindSummary>, DbErr>;
}
