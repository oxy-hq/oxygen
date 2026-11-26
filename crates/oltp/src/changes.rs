//! Capture for the OLTP change stream: the SQL that records every committed
//! change to a table an app declares under `oltpChanges`, in the same
//! transaction, into `oxy_changes.log` (platform step 8). Design:
//! `internal-docs/2026-11-22-oltp-change-stream-design.md` §5.1 and §5.2.
//!
//! A publish, after the app's migrations, on a session as the store's owner
//! pinned with [`OWNER_SESSION_PIN_SQL`]:
//! 1. reads each declared table's shape ([`SHAPE_SQL`], [`shape_from_rows`])
//!    and [`validate`]s every one before changing anything, so a refusal
//!    ([`DeclError`]) names each table at fault and removes nothing;
//! 2. diffs against what is installed ([`INSTALLED_SQL`]) and picks each
//!    table's [`next_schema_version`];
//! 3. per table, runs [`owner_install_sql`] as the owner in one transaction
//!    (with [`image_role_for`] the writer),
//!    [`writer_install_sql`] as the table's owner (the writer, or the owner
//!    for a table a workspace's schemas made), then [`owner_commit_sql`] as
//!    the owner;
//! 4. per table no longer declared, [`owner_uninstall_sql`],
//!    [`writer_uninstall_sql`] as the table's owner, [`owner_drop_sql`].
//!
//! Generating SQL is all this module does; it opens no connection. The
//! capture itself is four statement-level triggers per table calling four
//! definer functions the owner writes into the app's own schema, so the
//! writer can attach them but neither replace nor drop them, and the log
//! stays out of its reach. They build each image through their writer's
//! `oxy_changes.image_<writer>`, a definer owned by a role with no privileges, since
//! `to_jsonb` runs any cast to `json` an app defines on its own types
//! ([`image`]).
//!
//! A baseline restarts one table's current state from a fresh copy of its
//! rows, as a window in the same log: requested ([`request_baseline_sql`]),
//! begun ([`BEGIN_SQL`]), copied ([`CHUNK_WRITE_SQL`]) and completed
//! ([`COMPLETE_SQL`]), each lock taken by the writer's session and each log
//! row written by the owner's (design §5.5).

mod baseline;
mod capture;
mod image;
mod install;
mod relay;
mod shape;

/// What every branch cut and reset seeds a branch's change stream with
/// (`provisioner::branches::stream`): replace the branch's stream with one at
/// generation 0 that does not capture, and empty the log and the relay's
/// `progress` it inherited; nothing where step 8 (or step 9, for `progress`)
/// never ran.
pub const SEED_BRANCH_STREAM_SQL: &str = "DO $$ BEGIN \
     IF to_regclass('oxy_changes.stream') IS NOT NULL THEN \
     DELETE FROM oxy_changes.stream; \
     INSERT INTO oxy_changes.stream (generation, capturing) VALUES (0, false); \
     TRUNCATE oxy_changes.log; \
     END IF; \
     IF to_regclass('oxy_changes.progress') IS NOT NULL THEN \
     TRUNCATE oxy_changes.progress; \
     END IF; \
     IF to_regclass('oxy_changes.baselines') IS NOT NULL THEN \
     TRUNCATE oxy_changes.baselines; \
     END IF; \
     END $$";

pub use baseline::{
    ABANDON_ORPHANS_SQL, BACKLOG_SQL, BACKOFF_SQL, BEGIN_SQL, BaselineReason, CHUNK_READ_ISOLATION,
    CHUNK_WRITE_SQL, COMPLETE_SQL, DISCARD_SQL, KEY_TYPES_SQL, KeyOrder, LOCK_REQUEST_SQL,
    OPEN_BASELINES_SQL, PENDING_VIEWS_SQL, PLATFORM_ACTOR, REQUEST_MISSING_SQL, SnapshotShape,
    VIEW_PENDING_SQL, VIEW_RENDERED_SQL, abandon_baselines_sql, begin_lock_sql, chunk_read_sql,
    lock_timeout_sql, pin_session_sql, request_baseline_sql, request_named_baseline_sql,
};
pub use image::{OWNER_SESSION_PIN_SQL, image_function, image_role_for, image_role_prefix};
pub use install::{
    CaptureSpec, INSTALLED_SQL, owner_commit_sql, owner_drop_sql, owner_install_sql,
    owner_uninstall_sql, writer_install_sql, writer_uninstall_sql,
};
pub use relay::{
    CONFIRM_SQL, DECLARE_INTENT_SQL, DELETE_SHIPPED_SQL, LogRow, OLDER_GENERATION_SQL,
    PROGRESS_SQL, READ_BATCH_SQL, SHAPES_SQL, STREAM_SQL, StreamState, TRANSACTION_ROWS_SQL,
    WAITING_SQL,
};
pub use shape::{
    CaptureShape, Column, DeclError, DeclaredTable, RESERVED_COLUMNS, SHAPE_SQL, ShapeRow,
    TableShape, next_schema_version, shape_from_rows, validate,
};
