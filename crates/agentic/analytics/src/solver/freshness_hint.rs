//! Ambient data-freshness hint: a compact, runtime-computed line naming each
//! source's data boundary, injected into every LLM call as part of the same
//! uncached dynamic suffix that carries the "Today is ..." date hint.
//!
//! This is the zero-diligence complement to the `check_data_freshness` tool
//! (model-initiated, arbitrary depth) and the `freshness_check` solved rule
//! (post-execution guard): every state already knows how far each source's
//! data extends before writing a token, so recent-edge questions get scoped
//! correctly the first time - including scalar aggregates (YTD totals) whose
//! results carry no dates for the solved rule to inspect.
//!
//! Probes are `MAX(<watermark>)` per view - the same resolution the tool
//! uses (`Catalog::resolve_freshness_target`) - and results are cached
//! process-wide with a short TTL so per-run cost is amortized. Failures are
//! swallowed: the hint is best-effort and must never block the pipeline.
//!
//! Opt-in: only views that DECLARE a freshness contract in `meta:` are
//! included, so a workspace pays nothing (no probes, no tokens) until it
//! flags the views it deems freshness-sensitive. The tool keeps the
//! first-date-dimension fallback for explicit on-demand use.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use agentic_connector::DatabaseConnector;

use crate::catalog::Catalog;

/// Maximum number of views named in the hint (declared-contract views only).
const MAX_HINT_VIEWS: usize = 10;
/// How long a computed hint is reused before re-probing the warehouse.
const HINT_TTL: Duration = Duration::from_secs(60);

fn cache() -> &'static Mutex<HashMap<String, (Instant, Option<String>)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Option<String>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Compute the freshness hint for this catalog/connector set, or reuse a
/// recent cached value. Returns `None` when no view has a resolvable
/// freshness target or every probe fails.
pub async fn compute_freshness_hint(
    workspace_key: &str,
    catalog: &dyn Catalog,
    connectors: &HashMap<String, Arc<dyn DatabaseConnector>>,
    default_connector: &str,
) -> Option<String> {
    let mut names = catalog.table_names();
    names.sort();
    // workspace_key (the workspace path) scopes the process-wide cache per
    // tenant: two workspaces with identical view/table + connector names must
    // never share a cached coverage line in multi-tenant serve.
    let key = format!("{workspace_key}|{default_connector}|{}", names.join(","));

    if let Some((at, value)) = cache().lock().expect("poisoned").get(&key)
        && at.elapsed() < HINT_TTL
    {
        return value.clone();
    }

    // Ambient hint = opt-in via a DECLARED meta contract only. The tool keeps
    // resolve_freshness_target's first-date-dimension fallback for explicit
    // on-demand use, but the always-on hint holds itself to a higher bar: a
    // workspace pays nothing (no probes, no tokens) until it declares
    // freshness_watermark_column on the views it deems freshness-sensitive,
    // and every line the model sees then carries real cadence context.
    let mut targets: Vec<_> = names
        .iter()
        .filter_map(|n| catalog.resolve_freshness_target(n))
        .filter(|t| t.declared)
        .collect();
    targets.sort_by(|a, b| a.view.cmp(&b.view));
    targets.dedup_by(|a, b| a.view == b.view);
    targets.truncate(MAX_HINT_VIEWS);

    let futures: Vec<_> = targets
        .iter()
        .map(|t| async {
            // Skip the view rather than substitute the default. This probe's
            // result becomes an ambient "data through <date>" line the model
            // quotes to the user as fact, so measuring it on the wrong engine
            // is worse than not measuring it: a MAX(watermark) from a
            // different warehouse is a confident wrong date. Best-effort, so
            // dropping one view is the right failure -- the hint just omits it.
            let connector = match t.datasource.as_deref() {
                Some(name) => match super::lookup_connector(connectors, name).map(|(_, c)| c) {
                    Some(connector) => connector,
                    None => {
                        tracing::warn!(
                            view = %t.view,
                            datasource = name,
                            "skipping freshness probe: the view's datasource is not \
                             registered for this agent"
                        );
                        return None;
                    }
                },
                None => super::lookup_connector(connectors, default_connector).map(|(_, c)| c)?,
            };
            let table_sql = format!("\"{}\"", t.table.replace('"', "\"\""));
            let sql = format!("SELECT MAX({}) FROM {table_sql}", t.watermark_expr);
            let res = connector.execute_query(&sql, 1).await.ok()?;
            let cell = res.result.rows.first().and_then(|row| row.0.first())?;
            let through = match cell {
                agentic_core::result::CellValue::Text(s) => s.get(..10)?.to_string(),
                agentic_core::result::CellValue::Number(n) => {
                    let n = *n as i64;
                    if !(19000101..=29991231).contains(&n) {
                        return None;
                    }
                    format!("{:04}-{:02}-{:02}", n / 10000, (n / 100) % 100, n % 100)
                }
                agentic_core::result::CellValue::Null => return None,
            };
            match &t.expected_cadence {
                Some(c) => Some(format!("{} through {through} ({c})", t.view)),
                None => Some(format!("{} through {through}", t.view)),
            }
        })
        .collect();
    let lines: Vec<String> = futures::future::join_all(futures)
        .await
        .into_iter()
        .flatten()
        .collect();

    let hint = if lines.is_empty() {
        None
    } else {
        Some(format!("Data coverage: {}.", lines.join("; ")))
    };
    cache()
        .lock()
        .expect("poisoned")
        .insert(key, (Instant::now(), hint.clone()));
    hint
}
