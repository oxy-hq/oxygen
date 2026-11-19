//! [`TranscriptStore`] over `agentic_run_events`.

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use serde_json::Value;

use super::store::{StoreError, TranscriptStore};
use crate::lifecycle::crud::get_all_events;

pub struct DbTranscriptStore {
    db: DatabaseConnection,
}

impl DbTranscriptStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl TranscriptStore for DbTranscriptStore {
    async fn load(&self, run_id: &str) -> Result<Vec<(String, Value)>, StoreError> {
        let rows = get_all_events(&self.db, run_id)
            .await
            .map_err(|e| StoreError::Db(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|r| (r.event_type, r.payload))
            .collect())
    }
}
