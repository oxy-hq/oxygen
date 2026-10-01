//! A preview's shadow map (`workspace_preview_tables`): which live tables it
//! holds a copy of in its own schemas, and in what state. It doubles as the
//! record of every relation the preview made, which is what the TTL drop is
//! allowed to drop (`previews::drop`) — so it is written **before** a step's
//! SQL is sent ([`record_step`]), never after.

use airhouse::preview_sql::{CopyPlan, Rewrite, ShadowMap, ShadowState};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

/// Record what a step is about to do to the preview, before any of its SQL is
/// sent (host flow step 5, `registry` module doc): the map after the step
/// (`before`, then the step's `copies`, then its rewrite, as
/// `ShadowMap::apply_rewrite` settles them), and every entry that changed or
/// that the step writes, upserted. Returns the map after the step, for the
/// next step's rewrite.
///
/// Recording first is what makes a crash safe. A relation recorded but never
/// made is not there when the drop lists the schema, so nothing is sent for
/// it; a relation made but not yet recorded would be someone else's to the
/// drop, and keep the whole schema.
///
/// A table the step writes is upserted even when its state does not change,
/// so its row names this run as `last_run_id`: that is how a transform
/// build's compare finds every table the build wrote (`previews::compare`).
pub async fn record_step<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    preview_key: &str,
    run_id: &str,
    before: &ShadowMap,
    rewrite: &Rewrite,
    copies: &[CopyPlan],
) -> Result<ShadowMap, DbErr> {
    let mut after = before.clone();
    after.apply_rewrite(rewrite, copies);
    upsert_shadow(
        db,
        workspace_id,
        preview_key,
        run_id,
        &step_entries(before, &after, rewrite),
    )
    .await?;
    Ok(after)
}

/// [`shadow_changes`], plus every table `rewrite` writes whose state did not
/// change, at its state after the step. Sorted, each once.
fn step_entries(
    before: &ShadowMap,
    after: &ShadowMap,
    rewrite: &Rewrite,
) -> Vec<((String, String), ShadowState)> {
    let mut entries = shadow_changes(before, after);
    for live in &rewrite.writes {
        let unchanged = !entries.iter().any(|(l, _)| l == live);
        if let (true, Some(state)) = (unchanged, after.state(live)) {
            entries.push((live.clone(), state));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// The preview's shadow map: every live table it holds something for.
pub async fn load_shadow<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    preview_key: &str,
) -> Result<ShadowMap, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT live_schema, table_name, state FROM workspace_preview_tables \
             WHERE workspace_id = $1 AND preview_key = $2",
            [workspace_id.into(), preview_key.into()],
        ))
        .await?;
    let mut map = ShadowMap::default();
    for row in rows {
        let state: String = row.try_get("", "state")?;
        let Some(state) = parse_state(&state) else {
            return Err(DbErr::Custom(format!(
                "unknown preview table state {state:?}"
            )));
        };
        let live = (
            row.try_get("", "live_schema")?,
            row.try_get("", "table_name")?,
        );
        map.0.insert(live, state);
    }
    Ok(map)
}

/// What a step changed in the shadow map: every entry of `after` that is new
/// or in another state than in `before`, sorted. `ShadowMap::apply_rewrite`
/// only ever adds or re-states entries (a dropped table is `Dropped`, not
/// gone), so this is the whole change.
pub fn shadow_changes(
    before: &ShadowMap,
    after: &ShadowMap,
) -> Vec<((String, String), ShadowState)> {
    let mut changes: Vec<_> = after
        .0
        .iter()
        .filter(|(live, state)| before.state(live) != Some(**state))
        .map(|(live, state)| (live.clone(), *state))
        .collect();
    changes.sort_by(|a, b| a.0.cmp(&b.0));
    changes
}

/// Record shadow-map entries, in order (a later entry for the same table
/// wins, as [`ShadowMap::apply`] does) — normally [`shadow_changes`] after a
/// step. Pass a transaction to make the batch atomic. A relation the preview
/// created but did not record here is one the TTL drop will not drop, and it
/// keeps the whole schema from being dropped.
pub async fn upsert_shadow<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    preview_key: &str,
    run_id: &str,
    updates: &[((String, String), ShadowState)],
) -> Result<(), DbErr> {
    for ((live_schema, table), state) in updates {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO workspace_preview_tables \
                 (workspace_id, preview_key, live_schema, table_name, state, last_run_id) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (workspace_id, preview_key, live_schema, table_name) DO UPDATE SET \
                 state = EXCLUDED.state, last_run_id = EXCLUDED.last_run_id, updated_at = now()",
            [
                workspace_id.into(),
                preview_key.into(),
                live_schema.to_ascii_lowercase().into(),
                table.to_ascii_lowercase().into(),
                state_str(*state).into(),
                run_id.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

pub fn state_str(state: ShadowState) -> &'static str {
    match state {
        ShadowState::Shadow => "shadow",
        ShadowState::Partial => "partial",
        ShadowState::Sample => "sample",
        ShadowState::Dropped => "dropped",
    }
}

pub fn parse_state(state: &str) -> Option<ShadowState> {
    Some(match state {
        "shadow" => ShadowState::Shadow,
        "partial" => ShadowState::Partial,
        "sample" => ShadowState::Sample,
        "dropped" => ShadowState::Dropped,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shadow_state_round_trips_through_its_column_value() {
        for state in [
            ShadowState::Shadow,
            ShadowState::Partial,
            ShadowState::Sample,
            ShadowState::Dropped,
        ] {
            assert_eq!(parse_state(state_str(state)), Some(state));
        }
        assert_eq!(parse_state("Shadow"), None);
    }

    #[test]
    fn shadow_changes_are_the_new_and_restated_entries_only() {
        let t = |s: &str, n: &str| (s.to_string(), n.to_string());
        let mut before = ShadowMap::default();
        before.apply(&[
            (t("pos", "orders"), ShadowState::Shadow),
            (t("pos", "items"), ShadowState::Partial),
        ]);
        let mut after = before.clone();
        after.apply(&[
            (t("pos", "items"), ShadowState::Dropped),
            (t("gl", "entries"), ShadowState::Sample),
        ]);
        assert_eq!(
            shadow_changes(&before, &after),
            vec![
                (t("gl", "entries"), ShadowState::Sample),
                (t("pos", "items"), ShadowState::Dropped),
            ]
        );
        assert!(shadow_changes(&after, &after).is_empty());
    }

    /// A step writing a table the preview already holds changes no state,
    /// but its row must still name the step's run (a build's compare reads
    /// `last_run_id`).
    #[test]
    fn a_table_written_again_is_recorded_again() {
        use airhouse::preview_sql::{PreviewNamespace, RewriteOptions, rewrite};
        let ns = PreviewNamespace::from_key("feat_x_abc123").unwrap();
        let t = |s: &str, n: &str| (s.to_string(), n.to_string());
        let mut before = ShadowMap::default();
        before.apply(&[(t("pos", "orders"), ShadowState::Shadow)]);
        let step = rewrite(
            "INSERT INTO pos.orders SELECT 1",
            &ns,
            &before,
            &RewriteOptions::default(),
        )
        .unwrap();
        let mut after = before.clone();
        after.apply_rewrite(&step, &[]);
        assert!(shadow_changes(&before, &after).is_empty());
        assert_eq!(
            step_entries(&before, &after, &step),
            vec![(t("pos", "orders"), ShadowState::Shadow)]
        );
    }
}
