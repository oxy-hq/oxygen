//! `ctx.airhouse.query` and the writes behind it, under the `branch` and
//! `snapshot` modes (`env_policy::airhouse_overlay`,
//! `internal-docs/2026-11-02-custom-app-branches-design.md` §5.4).
//!
//! The host learns the mode from the policy's decision for the query, never
//! from the planes directly: `Allow` is `write-only` and runs the code in
//! `airhouse_ops` untouched; `Isolate(AirhouseOverlay | AirhouseSnapshot)` is
//! this module.
//!
//! A read is the app's production-form statement checked as production would
//! check it, then `overlay_reads`: a table the environment's shadow registry
//! says the sibling holds is read from the sibling, every other table live
//! from production. `snapshot` adds the pin, which must be the **last**
//! transformation: a pinned statement (`AT (TIMESTAMP => …)`) no longer parses
//! under our DuckDB-dialect parser, so nothing may check or rewrite it again.
//! It goes out as-is, on an app Reader, because a schema-scoped Writer cannot
//! time-travel.
//!
//! The copy that precedes a first write is `airhouse_overlay_copy`.

use airhouse::preview_sql::{self, PreviewNamespace, RewriteOptions, SnapshotPin, registry};
use airhouse::sql_rules::Access;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::super::env_policy::airhouse_overlay::{
    overlay_key, pin_refusal, reset_fix, snapshot_window,
};
use super::super::env_policy::homes::AirhouseWrite;
use super::super::env_policy::{self, Decision, HostOp, Target};
use super::airhouse_ops::{checked_statement, required_str};
use super::*;

/// The key under which the reader connection is cached beside the writers'
/// (`airhouse_conn` is keyed by schema, which never holds a colon).
const READER_KEY: &str = "reader:";

/// What the policy's query decision says the environment's reads are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverlayMode {
    Branch,
    Snapshot,
}

/// Fix text when the environment cannot name a sibling to overlay.
const NO_SIBLING_FIX: &str = "this environment has no Airhouse sibling for this writer name and \
     handle, so there is nothing to overlay; use a shorter handle or set airhouse=write-only";

impl ProjectFunctionHost {
    /// The overlay mode `ctx.airhouse` runs in, or `None` for `write-only`
    /// (and for production), whose path is unchanged.
    pub(super) fn overlay_mode(&self) -> Option<OverlayMode> {
        match self.policy.decide(HostOp::AirhouseQuery) {
            Decision::Isolate(Target::AirhouseOverlay) => Some(OverlayMode::Branch),
            Decision::Isolate(Target::AirhouseSnapshot) => Some(OverlayMode::Snapshot),
            _ => None,
        }
    }

    /// The environment's shadow-registry key, and the workspace it is in.
    pub(super) fn overlay_registry_key(&self) -> (Uuid, String) {
        let workspace = self.proj_ctx.workspace_manager().workspace_id;
        (
            workspace,
            overlay_key(self.app_id, self.policy.environment()),
        )
    }

    /// `ctx.airhouse.query` under `mode`: production's tables, with the
    /// tables the sibling holds read from it, and under `snapshot` the rest
    /// at the environment's pin.
    pub(super) async fn airhouse_overlay_query(
        &self,
        mode: OverlayMode,
        app_schema: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let target = (data_audit::PLANE_APP_AIRHOUSE, app_schema, "QUERY", "");
        let sibling = self.overlay_sibling(app_schema, target).await?;
        let checked = checked_statement(required_str(payload, "sql")?, app_schema, Access::Read)?;
        let ns =
            PreviewNamespace::for_environment(app_schema, &sibling).map_err(|e| e.to_string())?;
        let (workspace, key) = self.overlay_registry_key();
        let shadow = registry::load_shadow(&self.db, workspace, &key)
            .await
            .map_err(|e| format!("could not read the environment's Airhouse overlay: {e}"))?;
        let mut opts = overlay_options();
        let (connector, namespace) = match mode {
            OverlayMode::Branch => (self.airhouse_connector(&sibling).await?, sibling.as_str()),
            OverlayMode::Snapshot => {
                opts.pin = Some(self.snapshot_pin(target).await?);
                (self.airhouse_reader(app_schema).await?, app_schema)
            }
        };
        let sql =
            preview_sql::overlay_reads(&checked, &ns, &shadow, &opts).map_err(|e| e.to_string())?;
        data_audit::record_db_span(&db_query_summary(&checked), Some(namespace), None);
        let (rows, truncated) =
            query_with_truncation(&self.query_exec, connector, &sql, FUNCTION_MAX_ROWS).await?;
        let result = serde_json::json!({ "rows": rows, "truncated": truncated });
        enforce_result_byte_cap(&result)?;
        Ok(result)
    }

    /// The sibling the overlay sits on; the call is refused, and noted like
    /// any refusal, when the environment cannot name one.
    pub(super) async fn overlay_sibling(
        &self,
        app_schema: &str,
        target: HeldTarget<'_>,
    ) -> Result<String, String> {
        match self.policy.sibling_schema(app_schema) {
            Some(sibling) => Ok(sibling),
            None => Err(self.overlay_refusal(NO_SIBLING_FIX, target).await),
        }
    }

    async fn overlay_refusal(&self, fix: &str, target: HeldTarget<'_>) -> String {
        self.note_held(HostOp::AirhouseQuery, target).await;
        env_policy::refused_message(HostOp::AirhouseQuery, self.policy.environment(), fix)
    }

    /// The environment's pin, set by the first `snapshot` read when a reset
    /// has not set it. Refused when it is older than the window (or in the
    /// future), with the fix that re-pins it.
    async fn snapshot_pin(&self, target: HeldTarget<'_>) -> Result<SnapshotPin, String> {
        let environment = self.policy.environment();
        let pinned = self
            .resolve_pin()
            .await
            .map_err(|e| format!("could not read the environment's Airhouse pin: {e}"))?;
        let Some(pinned) = pinned else {
            let why = "this environment has no row to pin";
            return Err(self.overlay_refusal(why, target).await);
        };
        if let Some(why) = pin_refusal(pinned, Utc::now(), snapshot_window()) {
            let fix = format!("{why}; {}", reset_fix(&self.identity.app_slug, environment));
            return Err(self.overlay_refusal(&fix, target).await);
        }
        SnapshotPin::from_unix_micros(pinned.timestamp_micros()).map_err(|e| e.to_string())
    }

    /// `app_environments.airhouse_pinned_at`, set to the fork point (or now)
    /// by the first read when null. `None`: the environment has no row. The
    /// column is not on the entity, so it is read and written in raw SQL.
    async fn resolve_pin(&self) -> Result<Option<DateTime<Utc>>, sea_orm::DbErr> {
        let name = self.policy.environment().name();
        let ids = || [self.app_id.into(), name.clone().into()];
        let select = || {
            Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT airhouse_pinned_at FROM app_environments WHERE app_id = $1 AND name = $2",
                ids(),
            )
        };
        let Some(row) = self.db.query_one_raw(select()).await? else {
            return Ok(None);
        };
        if let Some(pinned) = row.try_get::<Option<DateTime<Utc>>>("", "airhouse_pinned_at")? {
            return Ok(Some(pinned));
        }
        // Now by this pod's clock, which `pin_refusal` checks against: a
        // database clock a little ahead would read as a pin in the future.
        let claim = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_environments SET airhouse_pinned_at = COALESCE(fork_at, $3) \
             WHERE app_id = $1 AND name = $2 AND airhouse_pinned_at IS NULL \
             RETURNING airhouse_pinned_at",
            [self.app_id.into(), name.clone().into(), Utc::now().into()],
        );
        // Nothing returned: a concurrent first read claimed it. Read theirs.
        let row = match self.db.query_one_raw(claim).await? {
            Some(row) => row,
            None => match self.db.query_one_raw(select()).await? {
                Some(row) => row,
                None => return Ok(None),
            },
        };
        row.try_get::<Option<DateTime<Utc>>>("", "airhouse_pinned_at")
    }

    /// The app's unscoped Reader, cached for this invocation.
    async fn airhouse_reader(
        &self,
        app_schema: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, String> {
        let key = format!("{READER_KEY}{app_schema}");
        let mut cached = self.airhouse_conn.lock().await;
        if let Some(connector) = cached.get(&key) {
            return Ok(connector.clone());
        }
        let connector = with_db_timeout("connect", async {
            self.proj_ctx
                .build_app_airhouse_reader(&self.identity.app_slug, app_schema)
                .await
                .map_err(|e| format!("could not connect to this app's Airhouse reader: {e}"))
        })
        .await?;
        cached.insert(key, connector.clone());
        Ok(connector)
    }

    /// Under `branch` and `snapshot`, a write the policy isolated to the
    /// sibling is rewritten to copy before its first write to a table
    /// (`airhouse_overlay_copy`). Any other write — `write-only`, or one
    /// production admitted as asked — is returned untouched, with nothing to
    /// settle.
    ///
    /// The second value is what the overlay holds once the write has run:
    /// pass it to [`Self::settle_overlay_write`] after it succeeds.
    pub(super) async fn route_overlay_write(
        &self,
        app_schema: &str,
        sql: &str,
        routed: AirhouseWrite,
    ) -> Result<(AirhouseWrite, OverlaySettle), String> {
        if self.overlay_mode().is_none() || routed.schema == app_schema {
            return Ok((routed, OverlaySettle::default()));
        }
        let sibling = routed.schema;
        let production_form = checked_statement(sql, app_schema, Access::Write)?;
        let ns =
            PreviewNamespace::for_environment(app_schema, &sibling).map_err(|e| e.to_string())?;
        let (workspace, key) = self.overlay_registry_key();
        let shadow = registry::load_shadow(&self.db, workspace, &key)
            .await
            .map_err(|e| format!("could not read the environment's Airhouse overlay: {e}"))?;
        let rewrite = preview_sql::rewrite(&production_form, &ns, &shadow, &overlay_options())
            .map_err(|e| e.to_string())?;
        self.ensure_overlay_copies(&sibling, &rewrite.preludes)
            .await?;
        // The rewritten statement is checked again, as the sibling's own:
        // exactly one statement, every write target in the sibling.
        let statement = checked_statement(&rewrite.sql, &sibling, Access::Write)?;
        Ok((
            AirhouseWrite {
                schema: sibling,
                statement,
            },
            OverlaySettle::after(rewrite),
        ))
    }

    /// After an overlay write succeeded: record each table it left the
    /// sibling holding that the registry does not list yet, so a query reads
    /// it back. That is a table production does not have (one a newer
    /// migration made, which no copy precedes) and one a `TRUNCATE` emptied
    /// in place. A table a copy brought over is already listed, with the
    /// copy's state (`partial` included), and is left as it is.
    pub(super) async fn settle_overlay_write(&self, settle: OverlaySettle) -> Result<(), String> {
        if settle.0.is_empty() {
            return Ok(());
        }
        let registry_err = |e: sea_orm::DbErr| format!("Airhouse overlay registry: {e}");
        let (workspace, key) = self.overlay_registry_key();
        let held = registry::load_shadow(&self.db, workspace, &key)
            .await
            .map_err(registry_err)?;
        let missing: Vec<_> = settle
            .0
            .into_iter()
            .filter(|(live, _)| held.state(live).is_none())
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let run_id = self.identity.invocation_id.to_string();
        registry::upsert_shadow(&self.db, workspace, &key, &run_id, &missing)
            .await
            .map_err(registry_err)
    }
}

/// The tables an overlay write leaves the sibling holding, recorded by
/// [`ProjectFunctionHost::settle_overlay_write`] once the write has run.
/// Empty for a write outside the overlay.
#[derive(Debug, Default)]
pub(super) struct OverlaySettle(Vec<((String, String), preview_sql::ShadowState)>);

impl OverlaySettle {
    /// What `rewrite` leaves the sibling holding: every table it writes, then
    /// its own updates (a `TRUNCATE`'s, say), which win.
    fn after(rewrite: preview_sql::Rewrite) -> Self {
        let mut held: Vec<_> = rewrite
            .writes
            .into_iter()
            .map(|live| (live, preview_sql::ShadowState::Shadow))
            .collect();
        for (live, state) in rewrite.shadow_updates {
            held.retain(|(seen, _)| *seen != live);
            held.push((live, state));
        }
        Self(held)
    }
}

/// The options every overlay statement is rewritten with: the deployment's
/// catalog, no live-only reads, and no pin (a read adds its own, last).
pub(super) fn overlay_options() -> RewriteOptions {
    RewriteOptions {
        catalog: airhouse::ducklake_catalog(),
        read_live_only: false,
        pin: None,
    }
}
