//! `ThreadOwnerLookup` impl backed by the platform `threads` table.
//!
//! Lives in the host application so the agentic stack stays free of the
//! `entity` crate.

use agentic_pipeline::platform::{ThreadOwnerLookup, ThreadScope};
use async_trait::async_trait;
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait};

pub struct OxyThreadOwnerLookup {
    db: DatabaseConnection,
}

impl OxyThreadOwnerLookup {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ThreadOwnerLookup for OxyThreadOwnerLookup {
    async fn thread_owner(
        &self,
        thread_id: uuid::Uuid,
    ) -> Result<Option<Option<uuid::Uuid>>, String> {
        entity::threads::Entity::find_by_id(thread_id)
            .one(&self.db)
            .await
            .map(|opt| opt.map(|t| t.user_id))
            .map_err(|e| e.to_string())
    }

    async fn thread_scope(&self, thread_id: uuid::Uuid) -> Result<Option<ThreadScope>, String> {
        entity::threads::Entity::find_by_id(thread_id)
            .one(&self.db)
            .await
            .map(|opt| {
                opt.map(|t| ThreadScope {
                    owner: t.user_id,
                    workspace_id: t.project_id,
                })
            })
            .map_err(|e| e.to_string())
    }

    async fn create_thread(
        &self,
        project_id: uuid::Uuid,
        title: &str,
    ) -> Result<uuid::Uuid, String> {
        let id = uuid::Uuid::new_v4();
        // Auto-provisioned thread for a run started without one (e.g. the
        // world-model chat). `user_id` is left null — this adapter has no
        // per-request user context, and the threads table allows it. The
        // run handler stamps the returned id onto `agentic_runs.thread_id`.
        let thread = entity::threads::ActiveModel {
            id: ActiveValue::Set(id),
            user_id: ActiveValue::Set(None),
            created_at: ActiveValue::not_set(),
            title: ActiveValue::Set(title.to_string()),
            input: ActiveValue::Set(title.to_string()),
            output: ActiveValue::Set(String::new()),
            source_type: ActiveValue::Set("analytics".to_string()),
            source: ActiveValue::Set("world-model".to_string()),
            references: ActiveValue::Set("[]".to_string()),
            is_processing: ActiveValue::Set(false),
            project_id: ActiveValue::Set(project_id),
            sandbox_info: ActiveValue::Set(None),
        };
        entity::threads::Entity::insert(thread)
            .exec(&self.db)
            .await
            .map_err(|e| e.to_string())?;
        Ok(id)
    }
}
