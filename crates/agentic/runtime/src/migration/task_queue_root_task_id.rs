//! `agentic_task_queue.root_task_id`: the root of the task tree a row is in.
//!
//! Every tree-scoped statement used to find a tree by its ids' shape,
//! `task_id = root OR task_id LIKE root || '.%'`. No index serves a `LIKE`
//! whose pattern is a parameter or an outer column, and `_` in a root id is a
//! wildcard. The tree is now a column, indexed, and those statements say
//! `root_task_id = $1`.
//!
//! **Derived from the run chain, not from the id's shape.** A child's run id
//! is its task id (`<parent>.<n>`), and its `agentic_runs` row names its
//! parent in `parent_run_id`, so walking `parent_run_id` up from a row's
//! `run_id` reaches the root run, whose id is the root task id. Nothing here
//! assumes a root id has no `.`. The walk's answer is kept only when the task
//! id really is that root or under it (`<root>.…`); a row whose run chain
//! names no such ancestor gets `split_part(task_id, '.', 1)`, which is what
//! the `LIKE` predicates selected for it. Production never takes that branch
//! (every root is enqueued with `task_id = run_id`); hand-built test rows do.
//!
//! **A trigger sets it, not the insert.** `agentic_task_queue_set_root_trigger`
//! fills `root_task_id` on every INSERT that leaves it NULL. So every insert
//! path is covered without each naming the root — `enqueue_task`'s upsert,
//! `requeue_task`'s raw INSERT, and the coordinator's re-assign of a
//! delegating node, which passes `parent_task_id: None` for a task that is not
//! a root. It also covers the previous deploy, which knows nothing of the
//! column, during a rolling deploy and after a revert, and is why the column
//! can be `NOT NULL`. An upsert's conflict path keeps the stored value: the
//! column is not in `enqueue_task`'s update list.
//!
//! **The backfill sends no wakes.** It is an UPDATE, so the AFTER INSERT OR
//! UPDATE `agentic_task_queue_notify_trigger` runs for every row, but its
//! function notifies only when `queue_status` becomes `queued`, and the
//! backfill never writes `queue_status`. It leaves `updated_at` alone too,
//! which retention and the compile backoff window read.
//!
//! Runs in the migrator's transaction (Postgres default), so the column, the
//! backfill and `SET NOT NULL` land together, and the index is a plain
//! `CREATE INDEX` (`CONCURRENTLY` cannot run in a transaction). `ADD COLUMN`
//! already holds the table's exclusive lock until commit.

use sea_orm_migration::prelude::*;

pub struct AddTaskQueueRootTaskId;

/// Named by hand: `DeriveMigrationName` takes the module path (see
/// `AddCompileTaskBackoffIndex`).
impl MigrationName for AddTaskQueueRootTaskId {
    fn name(&self) -> &str {
        "m20261105_000001_add_task_queue_root_task_id"
    }
}

/// The root of `p_task_id`, a row of run `p_run_id`. See the module docs.
/// The walk stops at 64 levels, so a cycle in `parent_run_id` cannot spin.
pub const ROOT_OF_FN_SQL: &str = "\
CREATE OR REPLACE FUNCTION agentic_task_queue_root_of(p_task_id TEXT, p_run_id TEXT) \
RETURNS TEXT LANGUAGE sql STABLE AS $$ \
    WITH RECURSIVE up(id, parent_run_id, depth) AS ( \
        SELECT id, parent_run_id, 0 FROM agentic_runs WHERE id = p_run_id \
        UNION ALL \
        SELECT r.id, r.parent_run_id, up.depth + 1 \
          FROM agentic_runs r JOIN up ON r.id = up.parent_run_id \
         WHERE up.depth < 64 \
    ) \
    SELECT COALESCE( \
        (SELECT id FROM up \
          WHERE p_task_id = id OR left(p_task_id, length(id) + 1) = id || '.' \
          ORDER BY depth DESC LIMIT 1), \
        split_part(p_task_id, '.', 1)) \
$$;";

const SET_ROOT_FN_SQL: &str = "\
CREATE OR REPLACE FUNCTION agentic_task_queue_set_root_fn() \
RETURNS TRIGGER AS $$ \
BEGIN \
    IF NEW.root_task_id IS NULL THEN \
        NEW.root_task_id := agentic_task_queue_root_of(NEW.task_id, NEW.run_id); \
    END IF; \
    RETURN NEW; \
END; \
$$ LANGUAGE plpgsql;";

const SET_ROOT_TRIGGER_SQL: &str = "\
CREATE TRIGGER agentic_task_queue_set_root_trigger \
BEFORE INSERT ON agentic_task_queue \
FOR EACH ROW EXECUTE FUNCTION agentic_task_queue_set_root_fn();";

const BACKFILL_SQL: &str = "\
UPDATE agentic_task_queue \
   SET root_task_id = agentic_task_queue_root_of(task_id, run_id) \
 WHERE root_task_id IS NULL";

pub const ROOT_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_task_queue_root_task_id \
     ON agentic_task_queue (root_task_id)";

#[async_trait::async_trait]
impl MigrationTrait for AddTaskQueueRootTaskId {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in [
            "ALTER TABLE agentic_task_queue ADD COLUMN IF NOT EXISTS root_task_id TEXT",
            ROOT_OF_FN_SQL,
            SET_ROOT_FN_SQL,
            "DROP TRIGGER IF EXISTS agentic_task_queue_set_root_trigger ON agentic_task_queue",
            SET_ROOT_TRIGGER_SQL,
            BACKFILL_SQL,
            "ALTER TABLE agentic_task_queue ALTER COLUMN root_task_id SET NOT NULL",
            ROOT_INDEX_SQL,
        ] {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for sql in [
            "DROP TRIGGER IF EXISTS agentic_task_queue_set_root_trigger ON agentic_task_queue",
            "DROP FUNCTION IF EXISTS agentic_task_queue_set_root_fn()",
            "DROP FUNCTION IF EXISTS agentic_task_queue_root_of(TEXT, TEXT)",
            "DROP INDEX IF EXISTS idx_task_queue_root_task_id",
            "ALTER TABLE agentic_task_queue DROP COLUMN IF EXISTS root_task_id",
        ] {
            db.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
