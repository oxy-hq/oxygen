//! Capture for the OLTP change stream: the SQL that records every committed
//! change to a table an app declares under `oltpChanges`, in the same
//! transaction, into `oxy_changes.log` (platform step 8). Design:
//! `internal-docs/2026-11-22-oltp-change-stream-design.md` §5.1 and §5.2.
//!
//! A publish, after the app's migrations:
//! 1. reads each declared table's shape ([`SHAPE_SQL`], [`shape_from_rows`])
//!    and [`validate`]s every one before changing anything, so a refusal
//!    ([`DeclError`]) names each table at fault and removes nothing;
//! 2. diffs against what is installed ([`INSTALLED_SQL`]) and picks each
//!    table's [`next_schema_version`];
//! 3. per table, runs [`owner_install_sql`] as the owner,
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
//! stays out of its reach.

mod capture;
mod install;
mod shape;

/// What every branch cut and reset seeds a branch's change stream with
/// (`provisioner::branches::stream`): replace the branch's stream with one at
/// generation 0 that does not capture, and empty the log it inherited; nothing
/// where step 8 never ran.
pub const SEED_BRANCH_STREAM_SQL: &str = "DO $$ BEGIN \
     IF to_regclass('oxy_changes.stream') IS NOT NULL THEN \
     DELETE FROM oxy_changes.stream; \
     INSERT INTO oxy_changes.stream (generation, capturing) VALUES (0, false); \
     TRUNCATE oxy_changes.log; \
     END IF; \
     END $$";

pub use install::{
    CaptureSpec, INSTALLED_SQL, owner_commit_sql, owner_drop_sql, owner_install_sql,
    owner_uninstall_sql, writer_install_sql, writer_uninstall_sql,
};
pub use shape::{
    CaptureShape, Column, DeclError, DeclaredTable, RESERVED_COLUMNS, SHAPE_SQL, ShapeRow,
    TableShape, next_schema_version, shape_from_rows, validate,
};
