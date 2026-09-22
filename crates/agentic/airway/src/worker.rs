//! Airway worker — runs a parsed [`AirwayPipelineSpec`] end-to-end and
//! bridges engine events onto an [`ExecutingTask`] channel pair the
//! agentic runtime can consume.
//!
//! Pattern B subsystem: one queue row → one engine run → done/failed.
//! No per-step decisions, no fan-out at the coordinator. Within a run,
//! resource-level fan-out happens inside [`airway::Pipeline::extract_source`]
//! via `extract_workers`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agentic_core::delegation::TaskOutcome;
use agentic_core::hub_task::spawn_with_hub;
use agentic_runtime::orchestrator::worker::ExecutingTask;
use airway::Pipeline;
use airway::airstack::{AirappEventHandler, EventBus, PipelineEvent};
use airway::connector::SourceContract;
use airway::state::StateStore;
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::boxed::{BoxedDestination, BoxedSourceConnector};
use crate::config::AirwayPipelineSpec;
use crate::destination_factory::build_destination;
use crate::error::AirwayError;
use crate::events::AirwayEvent;
use crate::source_factory::build_source_connector;
use crate::state_store::{AirwayPgStateStore, AirwayRunScopedStateStore};

/// Buffer for engine→runtime event forwarding. Sized so a burst of
/// resource completions in a wide pipeline doesn't make the engine
/// backpressure on the EventBus itself.
const EVENT_BUFFER: usize = 64;

/// Buffer for outcomes. Airway only ever produces a single terminal
/// outcome per run, but the runtime channel type wants a non-zero
/// capacity.
const OUTCOME_BUFFER: usize = 4;

/// Builds and drives a single airway pipeline run.
///
/// Construct once per dispatch (`agentic-pipeline`'s `TaskExecutor`
/// arm) and call [`AirwayWorker::execute`]. The returned
/// [`ExecutingTask`] mirrors what `agentic-runtime` expects — the
/// coordinator owns the channels from here.
#[derive(Clone)]
pub struct AirwayWorker {
    db: Arc<DatabaseConnection>,
    /// Optional QuickBooks token hook, supplied by the host: either a
    /// write-back sink for a grant this pipeline rotates, or a read-only
    /// access-token source for a grant some other writer owns. `None` for
    /// every other source. See [`crate::QuickBooksTokens`].
    tokens: Option<crate::QuickBooksTokens>,
    /// Optional credential provider, supplied by the host for
    /// `airhouse_managed` destinations so the destination re-mints a fresh
    /// (non-expired) ephemeral credential on every (re)connect. `None` for
    /// every other destination.
    credential_provider: Option<Arc<dyn crate::CredentialProvider>>,
    /// Contract-policy and environment admission this worker's runs are
    /// checked against at source construction. Defaults to `permissive` /
    /// `production` — today's behaviour.
    admission: crate::AirwayAdmission,
}

impl AirwayWorker {
    /// `admission` is a **required argument, not a builder step.** It decides
    /// whether a source is admitted at all, so the failure mode of an opt-in
    /// `with_admission` is a call site that forgets it — silently running under
    /// `permissive` / `production` while the deployment configured otherwise.
    /// Requiring it here makes that unrepresentable rather than merely untested;
    /// pass `AirwayAdmission::default()` for today's behaviour.
    pub fn new(db: Arc<DatabaseConnection>, admission: crate::AirwayAdmission) -> Self {
        Self {
            db,
            tokens: None,
            credential_provider: None,
            admission,
        }
    }

    /// Construct a worker that hands `sink` to the source factory so a
    /// rotated OAuth refresh token can be persisted to the host's secret
    /// store. Used by the executor for `quickbooks` pipelines this instance
    /// rotates.
    ///
    /// `admission` is required for the reason [`Self::new`] gives.
    pub fn with_refresh_sink(
        db: Arc<DatabaseConnection>,
        sink: Arc<dyn crate::RefreshTokenSink>,
        admission: crate::AirwayAdmission,
    ) -> Self {
        Self::with_quickbooks_tokens(db, crate::QuickBooksTokens::Rotating(sink), admission)
    }

    /// Construct a worker in **read-only** token custody: the source reads
    /// access tokens from `source` and never contacts Intuit's token endpoint.
    /// Used for grants whose rotation some other writer owns.
    ///
    /// `admission` is required for the reason [`Self::new`] gives.
    pub fn with_access_token_source(
        db: Arc<DatabaseConnection>,
        source: Arc<dyn crate::AccessTokenSource>,
        admission: crate::AirwayAdmission,
    ) -> Self {
        Self::with_quickbooks_tokens(db, crate::QuickBooksTokens::ReadOnly(source), admission)
    }

    fn with_quickbooks_tokens(
        db: Arc<DatabaseConnection>,
        tokens: crate::QuickBooksTokens,
        admission: crate::AirwayAdmission,
    ) -> Self {
        Self {
            db,
            tokens: Some(tokens),
            credential_provider: None,
            admission,
        }
    }

    /// Attach a [`crate::CredentialProvider`] handed to the destination factory
    /// so an `airhouse_managed` destination re-mints fresh credentials on every
    /// (re)connect. Chainable with the constructors above.
    pub fn with_credential_provider(
        mut self,
        provider: Arc<dyn crate::CredentialProvider>,
    ) -> Self {
        self.credential_provider = Some(provider);
        self
    }

    /// Start the airway run for `spec`. Returns immediately with
    /// receiver halves for events and outcomes; the actual extract /
    /// normalize / load runs on a spawned task.
    ///
    /// Errors are surfaced via [`TaskOutcome::Failed`] on the outcomes
    /// channel rather than as the function's return type — this keeps
    /// the worker shape uniform with how other domain executors plug
    /// into the runtime.
    /// `resume_run_id`: when `Some`, this run uses a RUN-SCOPED state store
    /// keyed by that run_id (persisting the cursor to
    /// `airway_run_extensions.resume_state`) instead of the pipeline-global
    /// store — set for resumable backfills so a reset-in-place retry resumes
    /// mid-window and the live pipeline cursor is never touched. `None` = normal
    /// run against the pipeline-global store.
    ///
    /// `run_id` is the owning `agentic_runs` id — used to stamp the
    /// engine-generated `load_id` onto the run extension once the load
    /// finishes. Distinct from `resume_run_id`, which is only `Some` for
    /// resumable backfills and selects the state store.
    ///
    /// `workspace_id` keys the cursor row the state store reads and writes. It
    /// must be the same id the caller used to take the single-flight lease —
    /// otherwise a run holds one workspace's lease while advancing another's
    /// cursor. `Uuid::nil()` in local mode and for `oxy airway run`, matching
    /// what the lease and the run row carry there.
    pub fn execute(
        &self,
        spec: AirwayPipelineSpec,
        resume_run_id: Option<String>,
        run_id: String,
        workspace_id: Uuid,
    ) -> ExecutingTask {
        let (event_tx, event_rx) = mpsc::channel::<(String, Value)>(EVENT_BUFFER);
        let (outcome_tx, outcome_rx) = mpsc::channel::<TaskOutcome>(OUTCOME_BUFFER);
        let cancel = CancellationToken::new();

        let db = self.db.clone();
        let tokens = self.tokens.clone();
        let credential_provider = self.credential_provider.clone();
        let admission = self.admission;
        let cancel_clone = cancel.clone();
        // If `drive` panics its JoinHandle is normally dropped and the
        // panic is swallowed: no `TaskOutcome` is ever sent and the
        // coordinator waits on a dead channel until cancel. Watch the
        // handle and synthesize a terminal `Failed` on panic/abort so
        // the run always reaches a terminal state.
        let outcome_tx_watch = outcome_tx.clone();
        // The lease release lives at the tail of `drive`, so a panic skips it —
        // and the pipeline would then be blocked for the full 6h TTL by the very
        // failure this watcher exists to convert into a terminal state. Clone
        // the handles the watcher needs to release it itself.
        let db_watch = self.db.clone();
        let run_id_watch = run_id.clone();
        spawn_with_hub(async move {
            let handle = spawn_with_hub(drive(
                spec,
                resume_run_id,
                run_id,
                workspace_id,
                db,
                tokens,
                credential_provider,
                admission,
                event_tx,
                cancel_clone,
                outcome_tx,
            ));
            if let Err(join_err) = handle.await {
                let msg = if join_err.is_panic() {
                    "airway worker panicked (internal error)".to_string()
                } else {
                    "airway worker task aborted".to_string()
                };
                // No-op if `drive` already sent an outcome before the
                // failure; this only fires when it never did.
                let _ = outcome_tx_watch.send(TaskOutcome::Failed(msg)).await;
                // Release here too: `drive`'s own release is inside the future
                // that just panicked. Idempotent — a DELETE scoped to this
                // run_id, so if `drive` did get far enough to release, this is
                // a no-op rather than a double-free of a successor's lease.
                if let Err(e) = crate::extension::pipeline_lease::release_by_run(
                    db_watch.as_ref(),
                    &run_id_watch,
                )
                .await
                {
                    warn!(run_id = %run_id_watch, error = %e,
                          "failed to release the airway lease after a worker panic; \
                           it will lapse at expires_at");
                }
            }
        });

        ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            // Airway never suspends mid-run, so no resume channel.
            answers: None,
        }
    }
}

/// Top-level driver spawned by [`AirwayWorker::execute`]. Owns every
/// piece of state for the run (no borrowed parameters), so the
/// returned future is straightforwardly `Send + 'static` for
/// `tokio::spawn`. Translates terminal outcome into the runtime's
/// [`TaskOutcome`] shape.
async fn drive(
    spec: AirwayPipelineSpec,
    resume_run_id: Option<String>,
    run_id: String,
    workspace_id: Uuid,
    db: Arc<DatabaseConnection>,
    tokens: Option<crate::QuickBooksTokens>,
    credential_provider: Option<Arc<dyn crate::CredentialProvider>>,
    admission: crate::AirwayAdmission,
    event_tx: mpsc::Sender<(String, Value)>,
    cancel: CancellationToken,
    outcome_tx: mpsc::Sender<TaskOutcome>,
) {
    let pipeline_name = spec.name.clone();
    // Set by the event forwarder once the engine emits its own
    // `pipeline_error`. Lets us tell "airway already reported the
    // failure on the stream" from "failed before any engine event"
    // (connector/destination build, secret resolution, state store) —
    // the latter would otherwise flip the run to failed with nothing
    // on the SSE stream, so the UI shows a status change but no cause.
    let saw_error = Arc::new(AtomicBool::new(false));
    let db_for_extension = db.clone();
    let outcome = match run_pipeline(
        spec,
        resume_run_id,
        workspace_id,
        db,
        tokens,
        credential_provider,
        admission,
        event_tx.clone(),
        cancel,
        saw_error.clone(),
    )
    .await
    {
        Ok(info) => {
            // Stamp the engine-generated load_id onto the extension row —
            // `insert_run_extension` leaves it NULL and documents the worker
            // as filling it in, which nothing did until now. Best-effort: a
            // failure here must not flip an otherwise-successful run.
            if let Err(e) = crate::extension::run_extension::set_run_load_id(
                db_for_extension.as_ref(),
                &run_id,
                &info.load_id,
            )
            .await
            {
                warn!(run_id = %run_id, load_id = %info.load_id, error = %e,
                      "failed to stamp load_id on the airway run extension");
            }
            // Carry the load_id and the fold outcomes out as task metadata —
            // now actually persisted, since `update_run_done` no longer
            // discards its patch. The folds are what let a consumer tell
            // "written" from "queryable" without replaying the event stream.
            let folds = serde_json::to_value(&info.folds).unwrap_or(serde_json::Value::Null);
            TaskOutcome::Done {
                answer: String::new(),
                metadata: Some(serde_json::json!({
                    "load_id": info.load_id,
                    "folds": folds,
                })),
            }
        }
        Err(err) => {
            if !saw_error.load(Ordering::Relaxed) {
                let domain = AirwayEvent::PipelineError {
                    pipeline_name,
                    load_id: None,
                    error: err.to_string(),
                };
                if let Ok(value) = serde_json::to_value(&domain) {
                    let _ = event_tx.send(("pipeline_error".to_string(), value)).await;
                }
            }
            TaskOutcome::Failed(err.to_string())
        }
    };

    // Release the single-flight lease on BOTH outcomes — a failed run must not
    // keep the pipeline blocked until the TTL lapses, or one bad load would
    // stall ingest for hours. Best-effort: the lease's `expires_at` is the
    // backstop if this DELETE cannot run (dead connection, killed pod), which
    // is exactly the case the TTL exists for.
    if let Err(e) =
        crate::extension::pipeline_lease::release_by_run(db_for_extension.as_ref(), &run_id).await
    {
        warn!(run_id = %run_id, error = %e,
              "failed to release the airway single-flight lease; it will lapse at expires_at");
    }

    let _ = outcome_tx.send(outcome).await;
}

/// Build the airway [`Pipeline`] from a spec and drive it to
/// completion. Pure airway-side; the runtime bridge lives in
/// [`AirwayWorker::execute`].
async fn run_pipeline(
    spec: AirwayPipelineSpec,
    resume_run_id: Option<String>,
    workspace_id: Uuid,
    db: Arc<DatabaseConnection>,
    tokens: Option<crate::QuickBooksTokens>,
    credential_provider: Option<Arc<dyn crate::CredentialProvider>>,
    admission: crate::AirwayAdmission,
    event_tx: mpsc::Sender<(String, Value)>,
    cancel: CancellationToken,
    saw_error: Arc<AtomicBool>,
) -> Result<airway::destination::LoadInfo, AirwayError> {
    // ── Install the deployment (operational) tier ──────────────────────────
    //
    // A *fallback* install. The primary one is at process boot, in `oxy-app`'s
    // `airway_boot` — airway's `GlobalConfig` is process-wide, so installing
    // there also covers the connector sites that never reach a run (source
    // discovery, policy preview). This call keeps `agentic-airway` correct for
    // a process with no such seam (an integration test, an embedder) and is
    // where a malformed row becomes a *run* failure the operator can read
    // rather than a boot-time warning. After a successful boot install it
    // short-circuits inside `install_once`'s `OnceCell` and logs nothing.
    //
    // **Within this crate this is the seam, and `AirwayWorker::new` is not.**
    // Three reasons, in the order they bite:
    //
    // 1. `new` runs once per *dispatch*, not once per process — the executor
    //    builds a worker for every queued run. airway's `install` is a
    //    `OnceLock`, so putting it there would turn "already installed" (a
    //    normal condition on run #2) into a diagnostic that means nothing, and
    //    the real second-installer case would be lost in the noise.
    // 2. `new` is sync and infallible, so it can neither await the row nor
    //    report a malformed one. Here the error joins the run's own failure
    //    path and reaches the operator on the SSE stream via `drive`.
    // 3. `new` is not even on every path — `with_refresh_sink` is a second
    //    constructor. Every construction funnels through `execute` → `drive` →
    //    here, so this is the one place that covers all of them.
    //
    // And it must be **before `build_source_connector`**: `HttpConfig::default`
    // and `RetryConfig::default` read the installed global, and every source
    // builds its client inside its constructor. Installing afterwards leaves
    // those clients on the built-in values with nothing to say so.
    //
    // `install_once` guards itself, so this line costs one `OnceCell` read per
    // run after the first.
    crate::deployment_config::install_once(db.as_ref()).await?;

    // ── Build pluggable parts ──────────────────────────────────────────────
    let connector = build_source_connector(&spec.source, tokens, admission.environment)?;
    // Read the **declared** contract map here, while the concrete connector is
    // still in hand, so the run's `pipeline_plan` can tell the operator how
    // each resource behaves. `contracts()`, not `contract_for()`: the latter
    // substitutes `SourceContract::default()` (= opaque) for anything
    // undeclared, which would report a gap as a checked vendor fact. Purely
    // observational — nothing here feeds admission, extraction, or writes.
    let declared_contracts = connector.contracts();
    let destination = build_destination(spec.destination.as_inline()?, credential_provider)?;

    // Admission runs **before** the source exists: `try_from_connector_with`
    // checks the contract policy and the environment and refuses rather than
    // handing back a usable handle to a source the deployment declined.
    // `from_connector` — the previous call — is `-> Self` and therefore has no
    // channel to refuse at all, which is why both policies were dark on oxy's
    // path regardless of what was configured.
    let mut source = airway::Source::try_from_connector_with(
        BoxedSourceConnector(connector),
        admission.contract_policy,
        admission.environment,
    )?;
    if !spec.resources.is_empty() {
        let names: Vec<&str> = spec.resources.iter().map(String::as_str).collect();
        source = source.with_resources(&names);
    }

    let state_store: Arc<dyn StateStore> = match resume_run_id {
        // Resumable backfill: run-scoped store — cursor → `resume_state` keyed by
        // run_id, schema + audit delegated to the pipeline-global row, live
        // cursor never touched.
        Some(run_id) => Arc::new(AirwayRunScopedStateStore::new(
            db,
            run_id,
            workspace_id,
            spec.name.clone(),
        )),
        None => Arc::new(AirwayPgStateStore::new(db, workspace_id, spec.name.clone())),
    };

    // ── Event bridge ──────────────────────────────────────────────────────
    let mut bus = EventBus::new();
    bus.subscribe(EventForwarder {
        tx: event_tx,
        saw_error,
        declared_contracts,
    });
    let bus = Arc::new(bus);

    // ── Compose pipeline ──────────────────────────────────────────────────
    //
    // `spec.concurrency` is threaded via `with_extract_workers`: airway
    // extracts resources sequentially when it's 1 and via
    // `buffer_unordered` otherwise (see `airway::Pipeline::extract_source`).
    let mut pipeline = Pipeline::new(spec.name.clone(), BoxedDestination(destination))
        .with_state_store(state_store)
        .with_event_bus(bus)
        .with_cancellation_token(cancel)
        .with_extract_workers(spec.concurrency)
        .with_streaming(spec.streaming);
    if let Some(cap) = spec.channel_capacity {
        pipeline = pipeline.with_channel_capacity(cap);
    }

    // Returned rather than discarded: `drive` stamps `load_id` onto the run
    // extension (documented as worker-filled, previously left NULL forever)
    // and carries it out as task metadata. `?` keeps the engine-error →
    // crate-error conversion that the previous `Ok(())` relied on.
    let info = pipeline.run_source(source).await?;
    Ok(info)
}

/// Subscriber on airway's `EventBus` that forwards every
/// [`PipelineEvent`] to the runtime event channel as a pre-serialised
/// `(event_type, payload)` pair.
///
/// Translates through [`AirwayEvent`] so the serialization contract
/// stays under oxy's control — the `event_type` discriminator and
/// payload field names are stable even if the engine struct evolves.
struct EventForwarder {
    tx: mpsc::Sender<(String, Value)>,
    /// Flipped once a `pipeline_error` is forwarded, so the driver
    /// knows the engine already reported the failure and doesn't
    /// double-emit a synthetic one.
    saw_error: Arc<AtomicBool>,
    /// The source connector's declared `SourceContract` map, captured in
    /// [`run_pipeline`] before the connector is boxed into the engine. The
    /// engine's `PipelinePlan` carries resource *names* only, so this is the
    /// one place that can attach how each resource behaves.
    declared_contracts: HashMap<String, SourceContract>,
}

#[async_trait]
impl AirappEventHandler for EventForwarder {
    async fn handle_event(&self, event: PipelineEvent) -> Result<(), airway::AirwayError> {
        let domain = AirwayEvent::from_engine(event, Some(&self.declared_contracts));
        match serde_json::to_value(&domain) {
            Ok(mut value) => {
                // Stamp emit time once, here (this handler fires when
                // the engine emits). It's persisted in the event
                // payload, so replay returns the same value — the
                // frontend reducer stays pure/idempotent and can build
                // a real time-axis run timeline. Injected at the
                // envelope level so all variants get it without
                // touching every `AirwayEvent` struct.
                if let Value::Object(map) = &mut value {
                    map.insert("ts".into(), Value::String(Utc::now().to_rfc3339()));
                } else {
                    // Every `AirwayEvent` variant is `#[serde(tag=...)]`
                    // so serializes as an object. If a future variant
                    // breaks that, the `ts` stamp (and the timeline)
                    // silently degrade — surface it instead of hiding
                    // behind the `unwrap_or("airway_event")` fallback.
                    debug_assert!(false, "AirwayEvent serialized as non-object: {value}");
                    warn!(value = %value, "AirwayEvent serialized as non-object; `ts` not stamped");
                }
                let event_type = value
                    .get("event_type")
                    .and_then(Value::as_str)
                    .unwrap_or("airway_event")
                    .to_string();
                if event_type == "pipeline_error" {
                    self.saw_error.store(true, Ordering::Relaxed);
                }
                if let Err(e) = self.tx.send((event_type, value)).await {
                    // Subscriber gone — the runtime stopped consuming
                    // events (cancellation, downstream drop). Log and
                    // continue; the pipeline shouldn't fail because of
                    // a closed SSE.
                    warn!(error = %e, "airway event channel closed; dropping events");
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to serialize AirwayEvent");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn forwarder(tx: mpsc::Sender<(String, Value)>) -> (EventForwarder, Arc<AtomicBool>) {
        forwarder_with(tx, HashMap::new())
    }

    fn forwarder_with(
        tx: mpsc::Sender<(String, Value)>,
        declared_contracts: HashMap<String, SourceContract>,
    ) -> (EventForwarder, Arc<AtomicBool>) {
        let saw_error = Arc::new(AtomicBool::new(false));
        (
            EventForwarder {
                tx,
                saw_error: saw_error.clone(),
                declared_contracts,
            },
            saw_error,
        )
    }

    #[tokio::test]
    async fn forwarder_pushes_serialised_event_type() {
        let (tx, mut rx) = mpsc::channel::<(String, Value)>(4);
        let (forwarder, saw_error) = forwarder(tx);
        forwarder
            .handle_event(PipelineEvent::LoadStarted {
                pipeline_name: "p".into(),
                load_id: "l".into(),
            })
            .await
            .expect("forward");

        let (event_type, payload) = rx.recv().await.expect("event");
        assert_eq!(event_type, "load_started");
        assert_eq!(payload["pipeline_name"], json!("p"));
        assert_eq!(payload["load_id"], json!("l"));
        assert!(
            !saw_error.load(Ordering::Relaxed),
            "non-error must not set saw_error"
        );
    }

    #[tokio::test]
    async fn forwarder_flags_saw_error_on_pipeline_error() {
        let (tx, mut rx) = mpsc::channel::<(String, Value)>(4);
        let (forwarder, saw_error) = forwarder(tx);
        forwarder
            .handle_event(PipelineEvent::PipelineError {
                pipeline_name: "p".into(),
                load_id: None,
                error: "boom".into(),
            })
            .await
            .expect("forward");
        let (event_type, _) = rx.recv().await.expect("event");
        assert_eq!(event_type, "pipeline_error");
        assert!(
            saw_error.load(Ordering::Relaxed),
            "pipeline_error must flip saw_error so drive() doesn't double-emit"
        );
    }

    /// The whole contract-display path in one assertion: the forwarder is the
    /// only place holding both the engine's plan and the connector's declared
    /// map, so if it stops projecting, the run UI silently loses the column.
    #[tokio::test]
    async fn forwarder_projects_contracts_onto_pipeline_plan() {
        let (tx, mut rx) = mpsc::channel::<(String, Value)>(4);
        let declared = HashMap::from([("orders".to_string(), SourceContract::immutable())]);
        let (forwarder, _saw_error) = forwarder_with(tx, declared);
        forwarder
            .handle_event(PipelineEvent::PipelinePlan {
                pipeline_name: "p".into(),
                load_id: "l".into(),
                resources: vec!["orders".into(), "users".into()],
                destination: "memory".into(),
            })
            .await
            .expect("forward");

        let (event_type, payload) = rx.recv().await.expect("event");
        assert_eq!(event_type, "pipeline_plan");
        assert_eq!(payload["contracts"][0]["resource"], json!("orders"));
        assert_eq!(payload["contracts"][0]["mutability"], json!("immutable"));
        // `users` is planned but undeclared — labelled, never defaulted to
        // `opaque` (which is what `contract_for` would have handed back).
        assert_eq!(payload["contracts"][1]["resource"], json!("users"));
        assert_eq!(payload["contracts"][1]["mutability"], json!("undeclared"));
    }

    #[tokio::test]
    async fn forwarder_silently_drops_on_closed_channel() {
        let (tx, rx) = mpsc::channel::<(String, Value)>(4);
        drop(rx); // close the receiver
        let (forwarder, _saw_error) = forwarder(tx);
        // Should not panic / not return Err — closed channel is logged
        // and the pipeline keeps running.
        forwarder
            .handle_event(PipelineEvent::PipelineError {
                pipeline_name: "p".into(),
                load_id: None,
                error: "boom".into(),
            })
            .await
            .expect("handle_event must not error on closed channel");
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::AirwayAdmission;
    use airway::connector::{ContractPolicy, Environment};

    fn db() -> Arc<sea_orm::DatabaseConnection> {
        // SeaORM 2.0 made `DatabaseConnection` a struct; the default is the
        // disconnected handle these admission tests want — they never query it.
        Arc::new(sea_orm::DatabaseConnection::default())
    }

    /// `AirwayAdmission::default()` is what a caller passes for today's
    /// behaviour; it must still be `permissive` / `production` after an
    /// upstream bump.
    #[test]
    fn the_default_admission_is_permissive_production() {
        let worker = AirwayWorker::new(db(), AirwayAdmission::default());
        assert_eq!(worker.admission.contract_policy, ContractPolicy::Permissive);
        assert_eq!(worker.admission.environment, Environment::Production);
    }

    #[test]
    fn both_constructors_carry_the_admission_they_were_given() {
        let admission = AirwayAdmission {
            contract_policy: ContractPolicy::RequireDeclared,
            environment: Environment::Sandbox,
        };
        assert_eq!(
            AirwayWorker::new(db(), admission).admission,
            admission,
            "new must carry it"
        );

        struct NoopSink;
        #[async_trait::async_trait]
        impl crate::RefreshTokenSink for NoopSink {
            async fn persist(&self, _token: &str) -> Result<(), String> {
                Ok(())
            }
        }
        assert_eq!(
            AirwayWorker::with_refresh_sink(db(), Arc::new(NoopSink), admission).admission,
            admission,
            "with_refresh_sink must carry it too — the quickbooks path"
        );
    }

    /// The chainable builder must not reset it: `with_credential_provider`
    /// rebuilds nothing, but a future one that did would silently drop the
    /// deployment's policy.
    #[test]
    fn a_chained_builder_preserves_the_admission() {
        let admission = AirwayAdmission {
            contract_policy: ContractPolicy::ForbidOpaque,
            environment: Environment::Production,
        };
        struct NoopProvider;
        #[async_trait::async_trait]
        impl crate::CredentialProvider for NoopProvider {
            async fn connection_string(&self) -> Result<String, String> {
                Ok(String::new())
            }
        }
        let worker =
            AirwayWorker::new(db(), admission).with_credential_provider(Arc::new(NoopProvider));
        assert_eq!(worker.admission, admission);
    }
}
