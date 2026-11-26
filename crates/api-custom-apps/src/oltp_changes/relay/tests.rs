//! The relay's constants and what a pass adds up.

use std::time::Duration;

use super::baseline::Baselines;
use super::pass::WriterPass;
use super::{
    BASELINE_STATEMENT_TIMEOUT, BATCH_CEILING, BATCH_ROWS, BATCHES_PER_WRITER, BEGIN_LOCK_TIMEOUT,
    COPY_BACKLOG, COPY_CHUNK_ROWS, INSERT_ROWS, LEASE_TTL, Limits, PASS_BUDGET, RELAY_KIND,
    RelayStats, SINK_TIMEOUT, STALE_AFTER,
};

#[test]
fn the_constants_are_the_designs() {
    assert_eq!(RELAY_KIND, "oltp_changes_relay");
    assert_eq!(BATCH_ROWS, 1000);
    assert_eq!(BATCH_CEILING, 50_000);
    assert_eq!(INSERT_ROWS, 500);
    assert_eq!(BATCHES_PER_WRITER, 5);
    assert_eq!(PASS_BUDGET, Duration::from_secs(60));
    assert_eq!(LEASE_TTL, Duration::from_secs(120));
    assert_eq!(SINK_TIMEOUT, Duration::from_secs(45));
    assert_eq!(STALE_AFTER, Duration::from_secs(300));
    assert_eq!(COPY_CHUNK_ROWS, 1000);
    assert_eq!(COPY_BACKLOG, 10_000);
    assert_eq!(BEGIN_LOCK_TIMEOUT, Duration::from_secs(1));
    assert_eq!(BASELINE_STATEMENT_TIMEOUT, Duration::from_secs(10));
}

#[test]
fn the_default_limits_are_the_constants() {
    assert_eq!(
        Limits::default(),
        Limits {
            batch_rows: BATCH_ROWS,
            batch_ceiling: BATCH_CEILING,
            batches_per_writer: BATCHES_PER_WRITER,
            budget: PASS_BUDGET,
            copy_chunk_rows: COPY_CHUNK_ROWS,
            copy_backlog: COPY_BACKLOG,
        }
    );
}

#[test]
fn a_pass_adds_up_its_writers_and_baselines() {
    let mut stats = RelayStats::default();
    stats.add(WriterPass {
        shipped: 7,
        batches: 2,
        held: 0,
        left: true,
    });
    stats.add(WriterPass {
        shipped: 0,
        batches: 0,
        held: 4,
        left: false,
    });
    stats.add_baselines(&Baselines {
        begun: 2,
        copied: 1500,
        completed: 1,
        open: true,
        failed: Vec::new(),
        wake_in: None,
    });
    assert_eq!(
        stats,
        RelayStats {
            shipped: 7,
            batches: 2,
            held: 4,
            begun: 2,
            copied: 1500,
            completed: 1,
            skipped_lease: false,
        }
    );
}
