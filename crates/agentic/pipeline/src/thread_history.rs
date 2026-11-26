//! The earlier turns of a thread an FSM analytics run is given.

use agentic_analytics::ConversationTurn;
use sea_orm::DatabaseConnection;

/// The last ten turns of `thread_id`, as the runtime reads them. None for a
/// run with no thread, or when the read fails.
pub(crate) async fn fsm_history(
    db: &DatabaseConnection,
    thread_id: Option<uuid::Uuid>,
) -> Vec<ConversationTurn> {
    let Some(tid) = thread_id else {
        return Vec::new();
    };
    let turns = agentic_runtime::crud::get_thread_history(db, tid, 10)
        .await
        .unwrap_or_default();
    turns
        .into_iter()
        .map(|t| ConversationTurn {
            question: t.question,
            answer: t.answer,
        })
        .collect()
}
