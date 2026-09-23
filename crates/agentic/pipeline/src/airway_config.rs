//! Resolve airway admission config for one run.
//!
//! Two rows may apply: the **global** row for a source kind
//! (`workspace_id IS NULL`) and a **sparse override** for one workspace. They
//! merge field by field, narrowest non-null winning — see [`resolve_admission`].
//!
//! The merge lives here, and only here, because it needs the `entity` crate:
//! `agentic-airway` (`crates/agentic/airway/CLAUDE.md`) and `agentic-core` are
//! both barred from depending on it, and this crate is explicitly allowed to.
//! The *type* it returns, [`ResolvedAdmission`], lives in `agentic-core`
//! beside the `TaskSpec::Airway` fields it fills, so the automation domain can
//! name the shape without reaching into this crate — it calls this
//! implementation through [`PipelineAirwayAdmissionResolver`], the facade-side
//! impl of the `agentic_automation::AirwayAdmissionResolver` port. That impl
//! is also where the transient-vs-determinate retry lives, because the
//! `DbErr` it classifies is only visible on this side of the port.

use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

use agentic_automation::WorkspaceContext;
use entity::airway_source_config;
use sea_orm::ExprTrait;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::db_transient::is_transient_db_error;
use crate::pipeline_ref::PipelineRefError;

/// The two admission strings for one run, as stored.
///
/// Re-exported from `agentic-core` so `agentic_pipeline::airway_config::
/// ResolvedAdmission` keeps resolving; there is exactly one definition and
/// exactly one merge (below).
pub use agentic_core::delegation::ResolvedAdmission;

/// Merge the global and per-workspace rows for `source_kind`.
///
/// **Field by field, not row by row.** A workspace row setting only
/// `environment` inherits `contract_policy` from the global row; taking the
/// workspace row wholesale would reset the omitted field to airway's default,
/// which is a policy downgrade nobody requested.
///
/// An absent table, an absent kind, and an all-null row are the same answer:
/// `None` for both, i.e. today's behaviour.
pub async fn resolve_admission(
    db: &DatabaseConnection,
    source_kind: &str,
    workspace_id: Uuid,
) -> Result<ResolvedAdmission, DbErr> {
    // One query, both candidate rows: the partial unique indexes guarantee at
    // most one of each, so this cannot return more than two.
    let rows = airway_source_config::Entity::find()
        .filter(airway_source_config::Column::SourceKind.eq(source_kind))
        .filter(
            airway_source_config::Column::WorkspaceId
                .is_null()
                .or(airway_source_config::Column::WorkspaceId.eq(workspace_id)),
        )
        .all(db)
        .await?;

    // Defence in depth: the partial unique indexes make duplicates impossible,
    // but if one were dropped the resolver would pick arbitrarily and the
    // effective policy would go non-deterministic. That must not be silent.
    warn_on_duplicates(&rows, source_kind, workspace_id);

    let global = rows.iter().find(|r| r.workspace_id.is_none());
    let scoped = rows.iter().find(|r| r.workspace_id == Some(workspace_id));

    let pick = |f: fn(&airway_source_config::Model) -> Option<String>| {
        scoped.and_then(f).or_else(|| global.and_then(f))
    };

    Ok(ResolvedAdmission {
        contract_policy: pick(|r| r.contract_policy.clone()),
        environment: pick(|r| r.environment.clone()),
    })
}

/// Warns, loudly and by name, if more than one row of either scope came back.
///
/// Split out so the resolver's happy path stays a straight read. Warn rather
/// than error: a duplicate means the resolved policy is arbitrary, not that it
/// is unsafe — refusing the run would convert a latent schema problem into an
/// outage, and the airway defaults the resolver falls back to are the
/// permissive ones the run would have had anyway.
fn warn_on_duplicates(rows: &[airway_source_config::Model], source_kind: &str, workspace_id: Uuid) {
    let globals = rows.iter().filter(|r| r.workspace_id.is_none()).count();
    if globals > 1 {
        tracing::warn!(
            source_kind,
            count = globals,
            "airway_source_config has {globals} global rows for source_kind `{source_kind}`; \
             airway_source_config_global_uniq should make this impossible. Resolving against an \
             arbitrary one — the effective admission policy for this kind is non-deterministic \
             until the duplicates are removed."
        );
    }

    let scoped = rows
        .iter()
        .filter(|r| r.workspace_id == Some(workspace_id))
        .count();
    if scoped > 1 {
        tracing::warn!(
            source_kind,
            %workspace_id,
            count = scoped,
            "airway_source_config has {scoped} rows for source_kind `{source_kind}` in workspace \
             {workspace_id}; airway_source_config_workspace_uniq should make this impossible. \
             Resolving against an arbitrary one — the effective admission policy for this \
             workspace is non-deterministic until the duplicates are removed."
        );
    }
}

/// Facade-side impl of the `agentic-automation` admission port.
///
/// An airway step inside an automation knows only its `pipeline_ref`; the
/// admission is keyed by `source_kind`, which lives in the pipeline YAML. This
/// closes that gap the same way [`crate::airway_run::start_airway_run`] does:
/// read the YAML through the compile boundary, parse it, take
/// `spec.source.kind`, then run the one [`resolve_admission`] merge. Handing
/// the domain a `pipeline_ref`-shaped port (rather than a `source_kind`-shaped
/// one) is what keeps the "where does `source_kind` come from" knowledge in a
/// single place.
pub struct PipelineAirwayAdmissionResolver {
    db: DatabaseConnection,
    workspace: Arc<dyn WorkspaceContext>,
    workspace_id: Uuid,
}

/// Which pipeline-YAML load failures are worth another attempt.
///
/// A free function rather than an inline `match` so the tests can exercise the
/// real mapping. While it was inline, the test module kept its own copy — and a
/// copy of the thing under test passes whatever the original does, so reverting
/// `Unavailable` to `Determinate` here would have left every one of those tests
/// green.
fn classify_load_failure(e: PipelineRefError) -> AttemptError {
    match e {
        PipelineRefError::Invalid(m) => AttemptError::Determinate(m),
        // `NotInRevision` is transient *here* for the same reason it is a
        // short deferral in the executor: a compile promoting the ref resolves
        // it, and this resolver's retries are already bounded by
        // `ADMISSION_MAX_ATTEMPTS`. Calling it determinate would refuse a
        // pipeline queued a moment before its compile landed.
        PipelineRefError::Io(m)
        | PipelineRefError::Unavailable(m)
        | PipelineRefError::NotInRevision(m) => AttemptError::Transient(m),
    }
}

impl PipelineAirwayAdmissionResolver {
    pub fn new(
        db: DatabaseConnection,
        workspace: Arc<dyn WorkspaceContext>,
        workspace_id: Uuid,
    ) -> Self {
        Self {
            db,
            workspace,
            workspace_id,
        }
    }
}

/// One attempt's failure, split by whether attempting again could change the
/// answer. This is the whole of the transient/determinate distinction — the
/// retry loop below does nothing but read this discriminant.
enum AttemptError {
    /// The effective policy genuinely cannot be known, and will still not be
    /// knowable in a second: the `pipeline_ref` doesn't resolve, the YAML
    /// doesn't parse, or the stored row is malformed. Retrying only delays a
    /// failure that is already final, so the step fails now.
    Determinate(String),
    /// Infrastructure failed, not the question: a connection reset, a pool
    /// timeout, a failover mid-statement. The same call a moment later is
    /// expected to succeed.
    Transient(String),
}

impl AttemptError {
    fn message(&self) -> &str {
        match self {
            AttemptError::Determinate(m) | AttemptError::Transient(m) => m,
        }
    }
}

/// Attempts, counting the first. Small on purpose — see the budget below.
const ADMISSION_MAX_ATTEMPTS: u32 = 3;
/// First backoff; doubles each retry (250ms, 500ms → 750ms of sleeping in the
/// worst case, which is the shape a dropped connection actually needs).
const ADMISSION_INITIAL_BACKOFF: Duration = Duration::from_millis(250);
/// Wall-clock ceiling on the whole retry sequence, checked before each sleep.
///
/// **This bound, not the attempt count, is the load-bearing one.** The
/// decision task's queue row carries `visibility_timeout_secs = 60`, its
/// `last_heartbeat` is stamped at claim time (`claim_task`), and the worker
/// only spawns the heartbeat loop *after* `executor.execute()` returns — so
/// every second spent inside this resolver is a second the row looks
/// unheartbeated. Overrun 60s and the reaper re-queues the task while it is
/// still running, and a second worker drives the same decision. The shared
/// pool's `acquire_timeout` is 30s, so a single pool-exhaustion failure can
/// eat 30s by itself; the budget is what stops us following it with another.
const ADMISSION_RETRY_BUDGET: Duration = Duration::from_secs(5);

impl PipelineAirwayAdmissionResolver {
    /// One resolution attempt, classified. Every failure mode this path has is
    /// enumerated here — the retry loop adds no classification of its own.
    async fn resolve_once(&self, pipeline_ref: &str) -> Result<ResolvedAdmission, AttemptError> {
        // Compile boundary first, workspace FS second — the same read
        // `start_airway_run` and the worker perform, so a stateless replica
        // resolves the admission without a working copy.
        //
        // `Unavailable` is the arm that carries this path: on a stateless
        // worker replica, a compile-boundary blip or a not-yet-compiled
        // revision means *this node cannot answer*, which no amount of
        // rewriting the ref would fix and which the next attempt very well
        // might. `Invalid` stays determinate — a ref that doesn't resolve
        // won't start resolving — and `Io` is a read that failed on a path
        // that did resolve, which is retryable but near-unreachable in
        // practice (see `PipelineRefError`).
        let yaml = crate::pipeline_ref::load_pipeline_yaml(self.workspace.as_ref(), pipeline_ref)
            .await
            .map_err(classify_load_failure)?;
        // `variables: None` matches what this dispatch path puts on the queued
        // `TaskSpec::Airway`, so the document parsed here is the document the
        // worker will parse. A parse failure is therefore a failure the run
        // would hit anyway — surfacing it at dispatch is strictly earlier. It
        // is also determinate by construction: the same bytes parse the same
        // way every time.
        let spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, None)
            .map_err(|e| AttemptError::Determinate(e.to_string()))?;
        resolve_admission(&self.db, &spec.source.kind, self.workspace_id)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if is_transient_db_error(&e) {
                    AttemptError::Transient(msg)
                } else {
                    AttemptError::Determinate(msg)
                }
            })
    }
}

#[async_trait::async_trait]
impl agentic_automation::AirwayAdmissionResolver for PipelineAirwayAdmissionResolver {
    /// Resolve, retrying only what retrying can fix.
    ///
    /// The port's contract is unchanged and deliberately so: an `Err` still
    /// fails the automation step, and a **determinate** failure still returns
    /// on the first attempt, so the property this whole surface exists for —
    /// a policy that cannot be known never queues under a silently-defaulted
    /// `permissive` — holds exactly as before. What changes is that a
    /// momentary database blip no longer counts as "cannot be known": it is
    /// re-attempted first, and only a blip that outlives
    /// [`ADMISSION_RETRY_BUDGET`] fails the run.
    ///
    /// **Why retry here rather than hand the queue a retryable error**: there
    /// is no such thing to hand it for this task. The durable queue's only
    /// failure-retry mechanism is `TaskPolicy::retry`, which
    /// `Coordinator::check_retry_or_fallback` applies solely to child tasks
    /// that carry a policy — and the root automation task is enqueued with
    /// `policy: None` (`automation_run.rs`), so it bails immediately.
    /// `claim_count` / `max_claims` is a redelivery lease for *dead workers*
    /// (the reaper only touches rows still `claimed`), not a failure budget.
    /// And the decider's `Fail` is written by `commit_decision` as one
    /// transaction over the run row and the queue row, so the run is terminal
    /// before the coordinator ever sees an outcome to reconsider. Making the
    /// queue retry this would mean attaching a `RetryPolicy` to every root
    /// automation task, which would also start re-driving deciders after
    /// genuine logic failures — a far larger change than this finding.
    async fn resolve_for_pipeline(&self, pipeline_ref: &str) -> Result<ResolvedAdmission, String> {
        retry_transient(pipeline_ref, || self.resolve_once(pipeline_ref)).await
    }
}

/// Drive `attempt` until it succeeds, fails determinately, or runs out of
/// patience. Free-standing and generic over the attempt so the policy can be
/// tested without a database or a workspace — the interesting behaviour is the
/// branching, not the query.
async fn retry_transient<F, Fut>(
    pipeline_ref: &str,
    mut attempt_fn: F,
) -> Result<ResolvedAdmission, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<ResolvedAdmission, AttemptError>>,
{
    // `tokio::time::Instant`, not `std::time::Instant`, so a test can
    // `start_paused` and exercise the budget without really sleeping.
    let started = Instant::now();
    let mut backoff = ADMISSION_INITIAL_BACKOFF;

    for attempt in 1..=ADMISSION_MAX_ATTEMPTS {
        let err = match attempt_fn().await {
            Ok(admission) => return Ok(admission),
            Err(e) => e,
        };

        // Determinate → out now, first attempt, message untouched. This is
        // the property the port exists for: no amount of patience makes an
        // unknowable policy knowable, and defaulting is not on the table.
        let AttemptError::Transient(msg) = &err else {
            return Err(err.message().to_string());
        };

        // Out of attempts, or the next sleep would push us past the budget:
        // give up now and let the step fail with the real cause attached.
        let elapsed = started.elapsed();
        if attempt == ADMISSION_MAX_ATTEMPTS || elapsed + backoff >= ADMISSION_RETRY_BUDGET {
            return Err(format!(
                "{msg} (transient; gave up after {attempt} attempt(s) in {elapsed:.1?})"
            ));
        }

        tracing::warn!(
            target: "airway_admission",
            pipeline_ref,
            attempt,
            backoff_ms = backoff.as_millis() as u64,
            error = %msg,
            "airway admission resolve failed transiently; retrying"
        );
        tokio::time::sleep(backoff).await;
        backoff *= 2;
    }

    // `ADMISSION_MAX_ATTEMPTS >= 1`, so the loop always returns.
    unreachable!("admission retry loop must return within ADMISSION_MAX_ATTEMPTS")
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::cell::Cell;

    fn transient() -> AttemptError {
        AttemptError::Transient("connection reset by peer".into())
    }

    /// A blip that clears is invisible to the automation: the step gets its
    /// policy, and the run that used to die here now proceeds.
    #[tokio::test(start_paused = true)]
    async fn a_transient_failure_that_clears_resolves() {
        let calls = Cell::new(0u32);
        let out = retry_transient("p.airway.yml", || {
            calls.set(calls.get() + 1);
            let fail = calls.get() < 3;
            async move {
                if fail {
                    Err(transient())
                } else {
                    Ok(ResolvedAdmission {
                        contract_policy: Some("strict".into()),
                        environment: Some("staging".into()),
                    })
                }
            }
        })
        .await
        .expect("a transient failure that clears must resolve");

        assert_eq!(out.contract_policy.as_deref(), Some("strict"));
        assert_eq!(out.environment.as_deref(), Some("staging"));
        assert_eq!(calls.get(), 3, "should have re-attempted, not given up");
    }

    /// The regression guard for the PR this fixes: a determinate failure must
    /// still fail the step, immediately, without burning the retry budget and
    /// **without** ever yielding a default admission.
    #[tokio::test(start_paused = true)]
    async fn a_determinate_failure_fails_on_the_first_attempt() {
        let calls = Cell::new(0u32);
        let err = retry_transient("nope.airway.yml", || {
            calls.set(calls.get() + 1);
            async { Err(AttemptError::Determinate("pipeline_ref not found".into())) }
        })
        .await
        .expect_err("a determinate failure must not resolve");

        assert_eq!(calls.get(), 1, "determinate failures must not be retried");
        assert_eq!(
            err, "pipeline_ref not found",
            "the determinate cause must reach the step verbatim"
        );
    }

    /// Patience is bounded: a blip that never clears still fails the step,
    /// carrying the underlying cause so an operator can tell it apart from a
    /// policy error.
    #[tokio::test(start_paused = true)]
    async fn a_transient_failure_that_never_clears_eventually_fails() {
        let calls = Cell::new(0u32);
        let err = retry_transient("p.airway.yml", || {
            calls.set(calls.get() + 1);
            async { Err(transient()) }
        })
        .await
        .expect_err("an unending transient failure must not resolve");

        assert_eq!(calls.get(), ADMISSION_MAX_ATTEMPTS);
        assert!(
            err.contains("connection reset by peer") && err.contains("transient"),
            "the cause and its classification must both survive: {err}"
        );
    }
}

#[cfg(test)]
mod resolution_classification_tests {
    use super::{AttemptError, classify_load_failure};
    use crate::pipeline_ref::{PipelineRefError, load_pipeline_yaml};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    struct Host {
        root: PathBuf,
        compiled: Result<Option<String>, String>,
    }

    #[async_trait::async_trait]
    impl agentic_automation::WorkspaceContext for Host {
        fn workspace_path(&self) -> Option<&Path> {
            Some(&self.root)
        }
        fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
            vec![]
        }
        async fn get_connector(
            &self,
            _name: &str,
        ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
            Err("unused".into())
        }
        async fn get_integration(
            &self,
            _name: &str,
        ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
            Err("unused".into())
        }
        async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
            Ok(vec![])
        }
        async fn resolve_automation_yaml(
            &self,
            _r: &str,
        ) -> Result<String, crate::WorkspaceReadError> {
            Err("unused".into())
        }
        async fn resolve_pipeline_yaml(&self, _r: &str) -> Result<Option<String>, String> {
            self.compiled.clone()
        }
    }

    /// The regression this exists for. A compile-boundary lookup that *fails*
    /// is not an answer, and on a worker replica there is no working copy to
    /// fall back to — so this used to arrive as `Invalid`, classify
    /// `Determinate`, and kill the automation run on attempt one.
    #[tokio::test]
    async fn a_compile_boundary_error_is_transient() {
        let host = Host {
            root: PathBuf::from("/nonexistent-oxy-workspace"),
            compiled: Err("pool timed out".into()),
        };
        let err = load_pipeline_yaml(&host, "p.airway.yml")
            .await
            .expect_err("a boundary error must not resolve");
        assert!(
            matches!(err, PipelineRefError::Unavailable(_)),
            "got {err:?}"
        );
        assert!(
            matches!(classify_load_failure(err), AttemptError::Transient(_)),
            "a boundary blip must be retried, not returned to the step"
        );
    }

    /// The other half, and the one product-context.md names: nothing compiled
    /// for this revision *yet* and no working copy here. Also not the caller's
    /// fault, also worth another attempt.
    #[tokio::test]
    async fn not_compiled_with_no_working_copy_is_transient() {
        let host = Host {
            root: PathBuf::from("/nonexistent-oxy-workspace"),
            compiled: Ok(None),
        };
        let err = load_pipeline_yaml(&host, "p.airway.yml")
            .await
            .expect_err("no row and no working copy must not resolve");
        assert!(
            matches!(err, PipelineRefError::Unavailable(_)),
            "got {err:?}"
        );
        assert!(matches!(
            classify_load_failure(err),
            AttemptError::Transient(_)
        ));
    }

    /// The line this must not blur: with a working copy present, a ref naming
    /// a file that isn't there is genuinely the caller's problem, and retrying
    /// it just delays the same answer.
    #[tokio::test]
    async fn a_missing_file_under_a_real_root_stays_determinate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = Host {
            root: dir.path().to_path_buf(),
            compiled: Ok(None),
        };
        let err = load_pipeline_yaml(&host, "nope.airway.yml")
            .await
            .expect_err("a missing file must not resolve");
        assert!(matches!(err, PipelineRefError::Invalid(_)), "got {err:?}");
        assert!(
            matches!(classify_load_failure(err), AttemptError::Determinate(_)),
            "a bad ref must still fail the step on the first attempt"
        );
    }
}
