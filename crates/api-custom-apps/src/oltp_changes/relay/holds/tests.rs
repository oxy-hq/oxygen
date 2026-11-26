//! A hold row's kind, as the relay reads it.

use super::HoldKind;

#[test]
fn a_row_keeps_the_kind_it_was_written_with() {
    assert_eq!(HoldKind::of_row("", "restore", "why"), HoldKind::Restore);
    assert_eq!(HoldKind::of_row("", "ceiling", "why"), HoldKind::Ceiling);
    assert_eq!(HoldKind::of_row("", "target", "why"), HoldKind::Target);
    assert_eq!(HoldKind::of_row("orders", "table", "why"), HoldKind::Table);
}

/// A writer's hold an older binary wrote takes the column's default,
/// `table`: its reason decides, as the migration's backfill did.
#[test]
fn an_old_binarys_writer_hold_is_classified_by_its_reason() {
    let restore = "restore detected (Rewound): a generation bump is needed (PR 3)";
    assert_eq!(HoldKind::of_row("", "table", restore), HoldKind::Restore);
    let ceiling = "a transaction of 60000 rows exceeds the relay's ceiling of 50000 \
                   (resync after PR 3)";
    assert_eq!(HoldKind::of_row("", "table", ceiling), HoldKind::Ceiling);
    let target = "the writer's app has no Airhouse";
    assert_eq!(HoldKind::of_row("", "table", target), HoldKind::Target);
    // A table's own hold is a table's, whatever its reason says.
    assert_eq!(
        HoldKind::of_row("orders", "table", restore),
        HoldKind::Table
    );
}
