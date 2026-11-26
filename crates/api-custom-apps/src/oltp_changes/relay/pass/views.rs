//! The current-state views a pass makes: a batch's, once the batch is
//! confirmed ([`after_confirm`]), and those an earlier render left pending
//! ([`rerender_pending`]). A view Airhouse refuses is recorded pending in the
//! store, on the table's latest complete baseline, so later passes make it
//! again until one succeeds, whether or not the table ships rows; one
//! Airhouse takes clears it. A pass with nothing pending makes no extra view.
//!
//! All of it is best effort: any error, the view's or the store's, logs
//! `oltp_changes_view_failed` and never fails a batch or a writer. Only a
//! lost lease stops it.

use std::collections::BTreeMap;

use tokio_postgres::Client;

use super::airhouse::{self, PendingView};
use super::target::{self, Target};
use super::{Pass, Stop, renew};
use crate::oltp_changes::relay::store;

/// Make each of a batch's `views`, once the batch is confirmed.
pub(super) async fn after_confirm(
    pass: &Pass<'_>,
    owner: &mut Client,
    target: &Target,
    views: &[PendingView],
) -> Result<(), Stop> {
    for view in views {
        make(pass, owner, target, view).await?;
    }
    Ok(())
}

/// Make `view`, after renewing the lease: its pending flag cleared when
/// Airhouse takes it, set when Airhouse refuses it. Any error is logged.
async fn make(
    pass: &Pass<'_>,
    owner: &mut Client,
    target: &Target,
    view: &PendingView,
) -> Result<(), Stop> {
    renew(pass).await?;
    let error = match airhouse::view(pass.sink, target, view).await {
        Ok(()) => store::view_rendered(owner, &target.name, &view.table)
            .await
            .err(),
        Err(refused) => match store::view_pending(owner, &target.name, &view.table).await {
            Ok(()) => Some(refused),
            Err(e) => Some(format!("{refused}; then {e}")),
        },
    };
    if let Some(error) = error {
        failed(pass, &target.name, &view.table, &error);
    }
    Ok(())
}

/// `oltp_changes_view_failed`, for `writer`'s `table` (empty for all of the
/// writer's, or of the store's).
fn failed(pass: &Pass<'_>, writer: &str, table: &str, error: &str) {
    alarm!(
        pass.environment,
        org_id = %pass.org_id,
        environment = pass.environment,
        writer,
        table,
        error,
        "oltp_changes_view_failed"
    );
}

/// Make again every view an earlier render left pending, writer by writer,
/// after the pass's shipping. A held writer's or table's waits for the hold;
/// a writer with no app or no Airhouse is held for that (`target::find`).
/// Any error is logged; a lost lease stops them all.
pub(in crate::oltp_changes::relay) async fn rerender_pending(
    pass: &Pass<'_>,
    owner: &mut Client,
) -> Result<(), Stop> {
    let pending = match store::pending_views(owner).await {
        Ok(pending) => pending,
        Err(error) => {
            failed(pass, "", "", &error);
            return Ok(());
        }
    };
    for (writer, tables) in by_writer(pending) {
        if pass.holds.writer(&writer) {
            continue;
        }
        match rerender_writer(pass, owner, &writer, &tables).await {
            Ok(()) => {}
            Err(stop @ Stop::Abort(_)) => return Err(stop),
            Err(Stop::Failed(error)) => failed(pass, &writer, "", &error),
        }
    }
    Ok(())
}

/// [`rerender_pending`] for one writer's `tables`.
async fn rerender_writer(
    pass: &Pass<'_>,
    owner: &mut Client,
    writer: &str,
    tables: &[String],
) -> Result<(), Stop> {
    let Some(target) = target::find(pass, writer).await? else {
        return Ok(());
    };
    let shapes = store::shapes(owner, writer).await.map_err(Stop::Failed)?;
    for table in tables.iter().filter(|t| !target.held.contains(t)) {
        let view = shapes
            .get(table)
            .and_then(|versions| PendingView::of(table, versions));
        if let Some(view) = view {
            make(pass, owner, &target, &view).await?;
        }
    }
    Ok(())
}

/// `(writer, table)` pairs grouped by writer.
fn by_writer(pending: Vec<(String, String)>) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (writer, table) in pending {
        grouped.entry(writer).or_default().push(table);
    }
    grouped
}
