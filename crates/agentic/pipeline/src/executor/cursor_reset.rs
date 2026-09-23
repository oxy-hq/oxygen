//! Rewind a pipeline's cursors, keeping everything it has landed.
//!
//! The non-destructive sibling of
//! [`reset_airway_schema`](super::PipelineTaskExecutor::reset_airway_schema),
//! which this file deliberately does not touch. That one drops the destination
//! tables and tombstones the state row; this one moves the cursors and nothing
//! else, so a resource can be re-pulled from an earlier `default_start` without
//! the pipeline's history being the price.
//!
//! Three things worth knowing at the call site:
//!
//! * **No destination is resolved.** The schema reset is airhouse-only because
//!   it has to re-mint a credential to issue the drop. Nothing is dropped here,
//!   so a cursor reset works against every destination kind, and cannot fail on
//!   a credential.
//! * **It can refuse.** Dropping the tables first makes every write disposition
//!   safe by construction. Keeping them does not: an append-only resource
//!   duplicates on a re-pull rather than converging, so for those this would be
//!   a silent data-corruption button. `agentic_airway::reset::convergence`
//!   judges that against the pipeline's stored schema, and this is where the
//!   verdict becomes a refusal.
//! * **It takes the pipeline's lease.** The clear bumps the state row's
//!   version, which is what makes it stick — and what makes a run in flight
//!   lose its next `save` wholesale. `Pipeline::persist` is best-effort, so
//!   that run drops *every* resource's advanced cursor, the append-only ones
//!   outside the scope included, and the next run re-pulls their window as
//!   duplicates. So a reset holds the same single-flight lease a run does:
//!   while a run holds it the reset refuses, and while the reset holds it a
//!   run defers. Taken, not checked — a read-only "is anything running?"
//!   leaves a window between the check and the write, and the whole harm fits
//!   in it.

use agentic_airway::extension::pipeline_lease::{self, LeaseAcquisition};
use agentic_airway::reset::cursor_reset_refusals;
use uuid::Uuid;

use super::PipelineTaskExecutor;

/// Re-exported so the transport layer can name this method's argument and
/// return types without depending on `agentic-airway`. `agentic-http` enters
/// agentic through this crate and only this crate; a route that reached past
/// it for a type would be the first exception, and exceptions to that rule are
/// how the layering stops being checkable.
pub use agentic_airway::reset::{ClearedCursors, CursorResetRefusal, CursorScope};

/// Error from [`PipelineTaskExecutor::reset_airway_cursors`].
///
/// The first three variants mirror
/// [`ResetSchemaError`](super::ResetSchemaError) so the two reset routes cannot
/// disagree about the same condition. [`Refused`](Self::Refused) is the one
/// this path adds, and it is not an error in the transport sense — the request
/// was well-formed and the server understood it; the operation would have
/// duplicated rows.
#[derive(Debug)]
pub enum ResetCursorsError {
    /// Bad/unknown `pipeline_ref` or an unparseable spec. → `400`.
    BadRequest(String),
    /// The state read or the cursor write failed — server-side. → `500`.
    Internal(String),
    /// The pipeline's YAML could not be resolved **on this node** — a
    /// compile-boundary blip, or a revision still compiling. → `503`.
    Unavailable(String),
    /// The convergence judgement refused. → `409`.
    ///
    /// Carries every reason, because an operator deciding whether to `force`
    /// needs all of them: a refusal that names one table of three invites an
    /// override made on a third of the facts. Carried typed rather than
    /// rendered, because the two kinds are different claims — "this would
    /// duplicate" and "I could not scope this" — and a client branching on
    /// the response must be able to tell them apart without reading prose.
    Refused(Vec<CursorResetRefusal>),
    /// Something else holds the pipeline's single-flight lease. → `409`.
    ///
    /// Usually a run, but `run_id` is the lease *holder*, which may also be
    /// another cursor reset (`cursor-reset:<uuid>`) or the acquire's
    /// contention sentinel ([`pipeline_lease::CONTENDED_HOLDER`]). The
    /// `Display` below branches on which through `LeaseHolder`; do not
    /// describe this variant as "a run" anywhere it could be one of the others.
    ///
    /// **Not overridable by `force`.** `force` overrides a *judgement* —
    /// convergence is inferred from a stored schema, and an operator may
    /// genuinely know better. A held lease is an observation about the
    /// present, and nobody knows better than it: forcing past a run makes
    /// its cursor save fail and drop every resource's progress, which is the
    /// duplicate-rows harm the refusal above exists to prevent. The answer is
    /// to wait for the holder — or cancel it, when it is a run.
    PipelineRunning { run_id: String },
}

impl std::fmt::Display for ResetCursorsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(m) | Self::Internal(m) | Self::Unavailable(m) => f.write_str(m),
            Self::Refused(refusals) => {
                let reasons: Vec<String> = refusals.iter().map(ToString::to_string).collect();
                write!(f, "cursor reset refused: {}", reasons.join("; "))
            }
            Self::PipelineRunning { run_id } => match LeaseHolder::of(run_id) {
                LeaseHolder::Run => write!(
                    f,
                    "cursor reset refused: run `{run_id}` of this pipeline is in flight. \
                     Resetting now would make that run's cursor save fail and drop every \
                     resource's progress, so its next run re-pulls those windows as duplicate \
                     rows. Wait for it to finish, or cancel it, then rewind"
                ),
                // No run to wait for or cancel, and no cursor save to fail —
                // only a lease that lives seconds.
                LeaseHolder::CursorReset => f.write_str(
                    "cursor reset refused: another cursor reset of this pipeline is in \
                     flight; retry in a moment",
                ),
                // All that was observed is that the acquire kept losing.
                LeaseHolder::Contended => f.write_str(
                    "cursor reset refused: could not take the pipeline lease after repeated \
                     attempts — something else is contending for it; retry in a moment",
                ),
                LeaseHolder::Unrecognised => write!(
                    f,
                    "cursor reset refused: the pipeline lease is held by `{run_id}`; retry \
                     once it is released"
                ),
            },
        }
    }
}

/// What a lease holder id says about who holds the lease — which is all the
/// refusal message may claim.
///
/// Each known shape is matched *positively*, and anything else is
/// [`Unrecognised`](Self::Unrecognised), whose message asserts nothing. A
/// "run unless proven otherwise" default is how a reset holder, then the
/// contention sentinel, came to be described as a run in flight; a new
/// sentinel now lands in the neutral arm instead.
enum LeaseHolder {
    /// An `agentic_runs` id — a v4 UUID (see `pipeline_lease::release_by_run`).
    Run,
    /// Another cursor reset's synthetic holder.
    CursorReset,
    /// `try_acquire` lost every race and found no row to name.
    Contended,
    Unrecognised,
}

impl LeaseHolder {
    fn of(holder: &str) -> Self {
        if holder.starts_with(CURSOR_RESET_HOLDER_PREFIX) {
            Self::CursorReset
        } else if holder == pipeline_lease::CONTENDED_HOLDER {
            Self::Contended
        } else if Uuid::parse_str(holder).is_ok() {
            Self::Run
        } else {
            Self::Unrecognised
        }
    }
}

impl std::error::Error for ResetCursorsError {}

/// How long a cursor reset's lease lives if the reset dies holding it.
///
/// A reset is two short statements, so a minute is generous; what matters is
/// the other direction. The synthetic holder names no run, so only this TTL
/// can reclaim it, and a run arriving meanwhile defers rather than fails.
const CURSOR_RESET_LEASE_TTL_SECS: i64 = 60;

/// Prefix of a cursor reset's synthetic lease holder. One constant, because
/// the refusal message branches on it: a holder minted in another spelling
/// would be described as a run.
const CURSOR_RESET_HOLDER_PREFIX: &str = "cursor-reset:";

impl PipelineTaskExecutor {
    /// Clear this workspace's incremental cursors for `pipeline_ref`, leaving
    /// the stored schema and every landed row intact. A same-named pipeline in
    /// another workspace is untouched.
    ///
    /// `scope` narrows it to named resources; `force` overrides a convergence
    /// refusal — never [`ResetCursorsError::PipelineRunning`].
    ///
    /// Composes the airway primitives in the order the refusal requires: load
    /// (which adopts the legacy shared row, so the judgement and the write both
    /// see the cursor a run would actually resume from), judge, then clear. The
    /// judgement is here rather than inside
    /// [`clear_pipeline_cursors`](agentic_airway::reset::clear_pipeline_cursors)
    /// because this is the layer that owns the error→status mapping, and a
    /// primitive that refuses is a primitive that cannot be reused by a caller
    /// with a different policy.
    pub async fn reset_airway_cursors(
        &self,
        pipeline_ref: &str,
        scope: &CursorScope,
        force: bool,
    ) -> Result<ClearedCursors, ResetCursorsError> {
        use ResetCursorsError::BadRequest;

        // Resolve `pipeline_ref` → yaml → spec, mirroring `reset_airway_schema`'s
        // first lines. No `variables`: a reset targets persisted state, keyed by
        // the rendered `name`.
        let yaml =
            match crate::pipeline_ref::load_pipeline_yaml(self.platform.as_ref(), pipeline_ref)
                .await
            {
                Ok(y) => y,
                Err(crate::pipeline_ref::PipelineRefError::Unavailable(m)) => {
                    return Err(ResetCursorsError::Unavailable(format!("airway: {m}")));
                }
                Err(e) => return Err(BadRequest(format!("airway: {e}"))),
            };
        let spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, None)
            .map_err(|e| BadRequest(format!("airway: parse `{pipeline_ref}`: {e}")))?;

        let pipeline_name = spec.name.clone();
        let workspace_id = self.platform.workspace_id();

        // A pipeline that opts out of single-flight takes no lease, so there
        // is no lease that could tell this reset a run is in flight — and
        // nothing else can: the clear's version bump is what would break an
        // in-flight run's save, not a guard against it. Unguarded, so say so
        // where an operator will look, not only here.
        let lease = if spec.allow_concurrent_runs {
            tracing::warn!(
                pipeline = %pipeline_name,
                workspace_id = %workspace_id,
                "cursor reset is UNGUARDED: this pipeline sets `allow_concurrent_runs: true`, \
                 so it takes no lease and a run in flight cannot be detected. If one is, \
                 its cursor save fails and drops every resource's progress, and its next \
                 run re-pulls those windows as duplicate rows",
            );
            None
        } else {
            Some(
                self.take_cursor_reset_lease(workspace_id, &pipeline_name)
                    .await?,
            )
        };
        let result = self
            .judge_and_clear_cursors(&spec, workspace_id, scope, force)
            .await;
        if let Some(holder) = lease {
            self.release_cursor_reset_lease(workspace_id, &pipeline_name, &holder)
                .await;
        }
        result
    }

    /// Load, judge, clear — under the lease [`reset_airway_cursors`] holds.
    ///
    /// [`reset_airway_cursors`]: Self::reset_airway_cursors
    async fn judge_and_clear_cursors(
        &self,
        spec: &agentic_airway::AirwayPipelineSpec,
        workspace_id: Uuid,
        scope: &CursorScope,
        force: bool,
    ) -> Result<ClearedCursors, ResetCursorsError> {
        use ResetCursorsError::Internal;
        let pipeline_name = &spec.name;

        // One load, used for both the judgement and the attribution universe.
        // Re-reading for the clear is fine (it is idempotent and adoption has
        // already happened); reading *only* there would not be.
        let snapshot =
            agentic_airway::reset::stored_cursor_state(&self.db, workspace_id, pipeline_name)
                .await
                .map_err(|e| Internal(e.to_string()))?;
        let held: Vec<String> = snapshot.state.resource_states.keys().cloned().collect();

        // `held` is what the scope resolves against; `spec.resources` joins it
        // only in the attribution universe — see `cursor_reset_refusals`.
        let refusals =
            cursor_reset_refusals(snapshot.schema.as_ref(), scope, &held, &spec.resources);
        if !refusals.is_empty() {
            if !force {
                return Err(ResetCursorsError::Refused(refusals));
            }
            // Echo what was overridden. A `force` is the operator asserting the
            // judgement is wrong for their case; the log is the only place that
            // assertion survives long enough to be checked against the row
            // counts afterwards.
            tracing::warn!(
                pipeline = %pipeline_name,
                workspace_id = %workspace_id,
                overridden = refusals.len(),
                reasons = %refusals.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "),
                "cursor reset forced past a convergence refusal — re-pulling these \
                 resources may duplicate rows rather than converge",
            );
        }

        agentic_airway::reset::clear_pipeline_cursors(&self.db, workspace_id, pipeline_name, scope)
            .await
            .map_err(|e| Internal(format!("reset clear-cursors: {e}")))
    }

    /// Take the pipeline's single-flight lease for the length of one reset.
    ///
    /// The holder id is synthetic (`cursor-reset:<uuid>`) and names no
    /// `agentic_runs` row, so acquisition's "holder reached a terminal status"
    /// reclaim can never free it: a reset that dies between here and the
    /// release is covered by the TTL alone. Hence
    /// [`CURSOR_RESET_LEASE_TTL_SECS`] rather than the six-hour run TTL — a
    /// crashed reset must not block the pipeline's schedule for an afternoon.
    async fn take_cursor_reset_lease(
        &self,
        workspace_id: Uuid,
        pipeline_name: &str,
    ) -> Result<String, ResetCursorsError> {
        let holder = format!("{CURSOR_RESET_HOLDER_PREFIX}{}", Uuid::new_v4());
        match pipeline_lease::try_acquire(
            &self.db,
            workspace_id,
            pipeline_name,
            &holder,
            CURSOR_RESET_LEASE_TTL_SECS,
        )
        .await
        .map_err(|e| ResetCursorsError::Internal(format!("cursor reset: take the lease: {e}")))?
        {
            LeaseAcquisition::Acquired => Ok(holder),
            LeaseAcquisition::Held { run_id, .. } => {
                Err(ResetCursorsError::PipelineRunning { run_id })
            }
        }
    }

    /// Release the reset's lease. Guarded on the holder id, so it can never
    /// free a lease a run took over after ours lapsed. A failure here is
    /// logged, not returned: the reset itself already happened (or was
    /// refused), and the TTL bounds how long a leaked lease can hold.
    async fn release_cursor_reset_lease(
        &self,
        workspace_id: Uuid,
        pipeline_name: &str,
        holder: &str,
    ) {
        if let Err(e) =
            pipeline_lease::release_counted(&self.db, workspace_id, pipeline_name, holder).await
        {
            tracing::warn!(
                pipeline = %pipeline_name,
                workspace_id = %workspace_id,
                error = %e,
                "cursor reset: releasing the pipeline lease failed; it lapses in \
                 {CURSOR_RESET_LEASE_TTL_SECS}s",
            );
        }
    }

    /// Resources holding a cursor for `pipeline_ref` in this workspace, sorted.
    ///
    /// The names [`reset_airway_cursors`](Self::reset_airway_cursors) accepts in
    /// a [`CursorScope::Resources`], served so a caller can *offer* them rather
    /// than ask for them.
    ///
    /// That distinction is the whole reason this exists. The scope wants the raw
    /// keys of `PipelineState::resource_states`, and the nearest thing a UI can
    /// otherwise reach — a table name from a run's lineage — is the resource
    /// name put through `NamingConvention::SnakeCase`. The two coincide for most
    /// pipelines and diverge silently for the rest (`vendorSales` holds the
    /// cursor; `vendor_sales` names the table), and a diverged name is reported
    /// in [`ClearedCursors::not_held`] *after* a reset that cleared nothing. A
    /// picker built from this list cannot make that mistake; one built from a
    /// lineage can, and would fail as a no-op on a control whose entire job is
    /// to be the safe alternative to dropping the tables.
    ///
    /// Errors mirror [`reset_airway_cursors`](Self::reset_airway_cursors)'s, so
    /// listing and resetting cannot disagree about an unresolvable
    /// `pipeline_ref`.
    pub async fn airway_resource_cursors(
        &self,
        pipeline_ref: &str,
    ) -> Result<Vec<String>, ResetCursorsError> {
        use ResetCursorsError::{BadRequest, Internal};

        let yaml =
            match crate::pipeline_ref::load_pipeline_yaml(self.platform.as_ref(), pipeline_ref)
                .await
            {
                Ok(y) => y,
                Err(crate::pipeline_ref::PipelineRefError::Unavailable(m)) => {
                    return Err(ResetCursorsError::Unavailable(format!("airway: {m}")));
                }
                Err(e) => return Err(BadRequest(format!("airway: {e}"))),
            };
        let spec = agentic_airway::AirwayPipelineSpec::from_yaml_with_vars(&yaml, None)
            .map_err(|e| BadRequest(format!("airway: parse `{pipeline_ref}`: {e}")))?;

        // Reads the legacy name-keyed row where this workspace has none — so
        // the picker lists the cursors a reset would then clear — without
        // adopting it: a listing is a read, and adoption is a write with a
        // mid-deploy cost. See `stored_resource_cursors`.
        agentic_airway::reset::stored_resource_cursors(
            &self.db,
            self.platform.workspace_id(),
            &spec.name,
        )
        .await
        .map_err(|e| Internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::ResetCursorsError;

    /// The lease holder can be a run, another reset, or the acquire's
    /// contention sentinel, and the refusal must say which. An operator told
    /// "a run is in flight" when none is goes looking for a run that does not
    /// exist.
    #[test]
    fn pipeline_running_names_what_actually_holds_the_lease() {
        let run_id = "0192f7a0-5c1e-4b7a-9f3d-2a6b8c4d1e0f";
        let by_run = ResetCursorsError::PipelineRunning {
            run_id: run_id.into(),
        }
        .to_string();
        assert!(
            by_run.contains(&format!("run `{run_id}` of this pipeline is in flight")),
            "{by_run}"
        );
        assert!(by_run.contains("cursor save fail"), "{by_run}");

        // Losing the acquire race repeatedly observes neither a run nor a
        // reset — only contention — so the message may claim neither.
        let contended = ResetCursorsError::PipelineRunning {
            run_id: "<unknown: lost the acquire race repeatedly>".into(),
        }
        .to_string();
        assert!(
            contended.contains("could not take the pipeline lease after repeated attempts"),
            "{contended}"
        );
        assert!(contended.contains("retry in a moment"), "{contended}");
        assert!(!contended.contains("run `"), "{contended}");
        assert!(!contended.contains("cursor reset of this"), "{contended}");

        // A holder of a shape nobody taught this message about must not be
        // described as a run: that fall-through is how the reset case was
        // wrong in the first place.
        let unrecognised = ResetCursorsError::PipelineRunning {
            run_id: "some-future-sentinel".into(),
        }
        .to_string();
        assert!(
            unrecognised.contains("`some-future-sentinel`"),
            "{unrecognised}"
        );
        assert!(!unrecognised.contains("run `"), "{unrecognised}");
        assert!(!unrecognised.contains("cursor save"), "{unrecognised}");

        let by_reset = ResetCursorsError::PipelineRunning {
            run_id: "cursor-reset:5b1c".into(),
        }
        .to_string();
        assert!(
            by_reset.contains("another cursor reset of this pipeline is in flight"),
            "{by_reset}"
        );
        assert!(by_reset.contains("retry in a moment"), "{by_reset}");
        // A reset has no cursor save to fail, and is not a run to cancel.
        assert!(!by_reset.contains("cursor save"), "{by_reset}");
        assert!(!by_reset.contains("cancel"), "{by_reset}");
    }
}
