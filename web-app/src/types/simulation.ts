/**
 * Declared worlds and their runs.
 *
 * Mirrors the DTOs in `crates/app/src/server/api/simulation.rs`. The one field
 * worth knowing about before reading any chart code: `coefficient` is `null`
 * **exactly** when the fit was refused. It is never `0` for a refusal — a zero
 * would erase the distinction the whole outcome taxonomy turns on, and a chart
 * that plots it as a point is drawing an estimate the model never made.
 */

/** One `.simulation.yml` as the revision compiled it. */
export interface SimulationSummary {
  name: string;
  file_path: string;
  /** The raw YAML body. Shape is the world's business, not the UI's. */
  definition: Record<string, unknown>;
}

/**
 * A candidate `.simulation.yml` body, as the create/edit form assembles it and
 * as `POST /simulations/validate` expects it. Mirrors
 * `crates/simulation/src/spec.rs::SimulationSpec` field-for-field — these are
 * the Rust struct's own field names, not relabeled for the UI, because this
 * object is posted as-is to `SimulationSpec::from_value` on the backend and
 * (once validated) serialized straight into the `.simulation.yml` file.
 */
export interface SimulationSpecInput {
  name: string;
  description?: string;
  seed: number;
  /** How many seeds this world is worth running at. Replicate 0 always runs
   *  the declared `seed`, so a single-replicate world is unaffected. */
  replicates: number;
  /** Decision periods the loop runs for. */
  periods: number;
  /** Days per decision period. */
  period_days: number;
  /** Days of history generated under the opening spend before the loop starts. */
  history_days: number;
  /** `YYYY-MM-DD`. */
  start_date: string;
  entities: EntitiesSpecInput;
  baseline: BaselineSpecInput;
  mechanism: MechanismSpecInput;
  lever: LeverSpecInput;
}

interface EntitiesSpecInput {
  /** Panels. `dof = n - (n_panels + k)`, so this is not free. */
  count: number;
  /** Log-space spread of entity size. */
  scale_sigma: number;
}

interface BaselineSpecInput {
  /** Sales for a typical entity on a typical day, before any marketing effect. */
  sales_per_entity_day: number;
  /** Contribution margin, in (0, 1). Sets where the profit optimum lands. */
  margin: number;
  /** AR(1) persistence of the latent demand shock, in (-1, 1). The confounder
   *  a `legacy` policy correlates spend with. */
  demand_shock_rho: number;
  demand_shock_sd: number;
  /** Amplitude of the weekly cycle, as a fraction of baseline. */
  weekly_seasonality: number;
  /** Idiosyncratic multiplicative spread on the budget rule — the
   *  identification axis. Zero is legal (the "flat lever" corner) but negative
   *  is not. */
  budget_jitter_sd: number;
}

/** Mirrors `DEFAULT_BUDGET_JITTER_SD` in `crates/simulation/src/spec.rs` —
 *  enough movement to identify a slope on an otherwise well-behaved world. */
export const DEFAULT_BUDGET_JITTER_SD = 0.12;

interface CalibrateSpecInput {
  /** Reference spend, as a share of baseline daily sales. */
  anchor_spend_share: number;
  /** Marginal sales per unit of spend, evaluated at the anchor spend. */
  local_slope_at_anchor: number;
  /** Where the profit optimum should sit, as a multiple of the anchor spend. */
  optimum_at: number;
}

export interface MechanismSpecInput {
  /** Bare column name — becomes the generated CSV column and the matching
   *  measure. Must match `^[A-Za-z_][A-Za-z0-9_]*$` (mirrors
   *  `is_bare_identifier` in `crates/simulation/src/spec.rs`: the value is
   *  interpolated raw into the CSV header and the generated `.view.yml`, so
   *  nothing outside that class survives unescaped). May not be
   *  `entity_id`/`date`/`prime_cost` (reserved by every generated world) or
   *  equal to `target`. */
  driver: string;
  /** Same constraints as `driver`. */
  target: string;
  /** Days between the spend and the sales it produces — the truth the world
   *  generates against. */
  lag_days: number;
  /** The lag the generated `.view.yml` claims, when it is not the true one.
   *  Leave unset when the customer guessed right. */
  declared_lag_days?: number | null;
  /** Noise on the target, as a fraction of the baseline level. */
  noise_ratio: number;
  calibrate: CalibrateSpecInput;
}

export interface LeverSpecInput {
  /** Floor on spend, as a multiple of the anchor. Must be > 0 (a zero floor is
   *  absorbing under a multiplicative step) and below `max_multiple`. */
  min_multiple: number;
  /** Ceiling on spend, as a multiple of the anchor. */
  max_multiple: number;
  /** Largest fractional change one decision period may make to spend, in (0, 1). */
  max_move_per_period: number;
  /** Log-space spread of the `machine+explore` jitter across entities. */
  explore_jitter_sd: number;
}

/** Mirrors `LeverSpec::default()` in `crates/simulation/src/spec.rs`. */
export const DEFAULT_LEVER: LeverSpecInput = {
  min_multiple: 0.1,
  max_multiple: 5.0,
  max_move_per_period: 0.25,
  explore_jitter_sd: 0.15
};

/** Reserved by every generated world's view/CSV (`world_dir::view_yml`) —
 *  `driver`/`target` may not collide with these. */
export const RESERVED_COLUMN_NAMES = ["entity_id", "date", "prime_cost"] as const;

/** Mirrors `MIN_FIT_OBSERVATIONS` in the airlayer fitter — used to compute a
 *  live "N paired observations" hint. The backend `validate` call is still the
 *  authority; this only lets the form warn before that round trip. */
export const MIN_FIT_OBSERVATIONS = 30;

export interface ValidateResponse {
  ok: boolean;
  error?: string;
}

/** The five arms a run can be. A property of the RUN, not of the world — the
 *  same `.simulation.yml` has to be runnable under all of them or a profit race
 *  compares two worlds that happen to look alike. */
export type Policy = "hold" | "legacy" | "machine" | "machine_explore" | "oracle";

export const POLICIES: Policy[] = ["hold", "legacy", "machine", "machine_explore", "oracle"];

interface EnqueuedRun {
  run_id: string;
  simulation: string;
  policy: Policy;
  /** Which draw of the world this is; `0` is the seed the file declares. */
  replicate: number;
  /** The seed this run got. Goes out as a JSON number of a Rust `u64`, so a
   *  declared seed above 2^53 does NOT survive the parse exactly — reproduce
   *  such a draw from the run's `spec`, or from the `.simulation.yml`, not from
   *  this field. Seeds in the range anyone types by hand are exact. */
  seed: number;
}

/**
 * What `POST /simulations/{name}/runs` answers with.
 *
 * Each run is queued in its own transaction, so a failure part-way leaves the
 * earlier arms queued AND executing. `partial_failure` is non-null exactly
 * then: `runs` are real and running, and the note names what did not happen.
 * A request that queued nothing is an HTTP error, never an empty `runs`.
 */
export interface QueuedRuns {
  runs: EnqueuedRun[];
  partial_failure: string | null;
}

/** `?limit=&offset=` on `GET /simulations/runs`. Limit defaults to 100 and is
 *  clamped to 1000 server-side. */
export interface RunListPage {
  limit?: number;
  offset?: number;
}

type RunStatus = "queued" | "running" | "done" | "failed" | "cancelled";

export interface SimulationRun {
  run_id: string;
  workspace_id: string;
  revision_id: string | null;
  simulation_name: string;
  policy: Policy;
  /** Same 2^53 caveat as `EnqueuedRun.seed`. */
  seed: number;
  /** Runs of one `(simulation_name, policy)` across replicates are the same
   *  experiment repeated. A cell of the outcome map is their aggregate, never a
   *  single one of them — a marginal world's single draw is a coin toss. */
  replicate: number;
  status: RunStatus;
  spec: Record<string, unknown>;
  /**
   * The world's true parameters. The one place truth is recorded — written when
   * the run finishes, so it is `null` while a run is still going and the truth
   * line cannot be drawn yet.
   */
  truth: { theta: number; scale: number; anchor_spend: number; optimum_spend: number } | null;
  periods_planned: number;
  periods_done: number;
  /** When the run was enqueued. Listings are ordered on this. */
  queued_at: string;
  /** When a worker claimed the run. Equal to `queued_at` until one does, so
   *  `finished_at - started_at` is runtime and `started_at - queued_at` is
   *  queue wait. */
  started_at: string;
  finished_at: string | null;
  error: string | null;
}

export interface SimulationPeriod {
  run_id: string;
  period: number;
  mean_spend: number;
  realized_profit: number;
  cumulative_profit: number;
  /** Per-entity spend. A mean cannot show whether an `explore` arm left any
   *  variation behind, which is the only question that arm exists to answer. */
  actions: number[];
}

/**
 * The three outcomes a run distinguishes.
 *
 * `converged` and `confidently_wrong` are the same response on real data — only
 * a world whose answer we chose can tell them apart, which is the entire reason
 * the simulation exists. Colour them accordingly.
 */
export type Outcome = "refused" | "converged" | "confidently_wrong";

export interface SimulationFit {
  run_id: string;
  period: number;
  /** `driver -> target`. */
  edge: string;
  /** The basis the fitter chose (`linear`, `log-log`, …). A coefficient is
   *  meaningless without it — an elasticity read as a level slope is wrong by
   *  `target / driver`. */
  form: string;
  /** `null` exactly on a refusal. Never plot this as zero. */
  coefficient: number | null;
  se: number | null;
  t_stat: number | null;
  n: number;
  n_panels: number;
  refusal: string | null;
  /** The truth this period is scored against, at the spend the world actually
   *  settled at — not at the anchor its curve was calibrated from. */
  true_local_slope: number;
  outcome: Outcome;
}

export interface RunDetail {
  run: SimulationRun;
  periods: SimulationPeriod[];
  fits: SimulationFit[];
}
