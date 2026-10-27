/**
 * Wire types for the `/experiments*` routes. They are the backend plans' own:
 * index C5 "Response and request shapes" (list, proposals, price, abort,
 * write-back), PR 4 Task 13 (`GET /run`) and PR 5 Task 8 (`GET /result`), each
 * grown as C5's closing note says. The note names the content of the growth but
 * not its field names, so the names of `RunDetail`'s growth below are fixed by index C5 (Contract issue 22); the result types are PR 5 Task 8's verbatim (issue 23).
 *
 * `RunDetail` has no effect field on purpose: the API returns none before
 * `reported`, and the UI must not grow a way to ask for one. Timestamps are
 * RFC 3339, dates are `YYYY-MM-DD` in the experiment's `design.timezone`.
 * `null` in an `Interval` is an unbounded endpoint (`serde_json` writes ±∞ as null).
 */

export type RunState = "registered" | "live" | "closing" | "reported" | "aborted";

/** Every experiment has a status; only some have a run — `RunState` is a subset. */
export type ExperimentStatus = "draft" | "registrable" | "registration_refused" | RunState;

export type ProposalStatus = "pending" | "done" | "refused" | "failed";

// ---- POST /price, GET /proposals, GET /experiments (index C5) ----------------

/** A staggered design: waves of units, or of groups under `assign_by`. */
export type WavesCandidate = {
  shape: "waves";
  wave_sizes: number[];
  first_switch_on: string;
  spacing_days: number;
  /** 2 or more stratifies by baseline volume; omitted = unblocked. */
  blocks?: number;
  /** Matched pairs: the server pairs units by baseline and treats one of each pair. */
  pairs?: boolean;
  /** A dimension of `for_each`: randomise its values (groups), not single units. */
  assign_by?: string;
  /** Overrides `outcome.alpha` for this candidate (the directional preset is 0.2). */
  alpha?: number;
};

/** The whole fleet alternates on and off over `pairs` consecutive pairs of `period_days`. */
export type SwitchbackCandidate = {
  shape: "switchback";
  first_period_on: string;
  period_days: number;
  pairs: number;
  alpha?: number;
};

export type Candidate = WavesCandidate | SwitchbackCandidate;

/** A request prices 1 to 6 candidates; each becomes its own proposal. */
export interface PriceRequest {
  candidates: Candidate[];
}

export interface PriceResponse {
  /** Groups the candidates of this request in the proposal log. */
  request_id: string;
  /** One `pending` row per candidate, in the order they were sent. */
  proposal_ids: string[];
}

export interface ProposedWave {
  switch_on: string;
  units: string[];
}

export interface ProposedPeriod {
  from: string;
  state: "on" | "off";
}

export type PriorSource = "declared" | "fitted";

export interface PriorCoefficient {
  value: number;
  source: PriorSource;
}

/** One logged candidate. `refusal` carries the reason for `refused` AND `failed`. */
export interface Proposal {
  id: string;
  request_id: string;
  status: ProposalStatus;
  created_at: string;
  finished_at: string | null;
  request: Candidate;
  seed: number | null;
  waves: ProposedWave[] | null;
  /** A switchback's schedule: consecutive pairs, one on and one off each. */
  periods: ProposedPeriod[] | null;
  /** The blocks the assignment was randomised within. */
  strata: string[][] | null;
  assign_by: string | null;
  /** Minimum detectable effect in the design's OWN unit: $/unit-day, or $/group-day under `assign_by`. */
  mde: number | null;
  /** `mde / baseline` as a fraction (0.052 is 5.2%). */
  mde_relative: number | null;
  baseline: number | null;
  /** Comparable across candidates: $/unit-day (index C2b). The only scale candidates are ranked on. */
  mde_unit_day: number | null;
  /** The α this candidate was priced at. */
  alpha: number;
  /** The World Model's slope on the driver edge, frozen into the file; null = no slope existed. */
  prior_coefficient: PriorCoefficient | null;
  warnings: string[];
  refusal: string | null;
  /** The keys pricing produced, as file text. On the wire, never displayed: the UI describes them in words. */
  snippet: string | null;
  /** The design this proposal priced; null on rows from before designs were records. */
  design_id: string | null;
  /** The draft was edited after this proposal priced it, so it can no longer be used. */
  stale: boolean;
}

export type ProposalSummary = Pick<
  Proposal,
  "id" | "request_id" | "status" | "created_at" | "mde" | "mde_unit_day" | "refusal"
>;

export type LeverRef = { via: string };

/**
 * The compiled `ExperimentConfig` (crates/experiment/src/config.rs) as JSON, defaults filled in.
 * Unknown extra keys are allowed: the Definition view renders whatever arrives.
 */
/** Keeps the rows of a measure's view whose dimension is (or is not) one of the values. */
export interface MeasureFilter {
  dimension: string;
  op: "equals" | "not_equals";
  values: string[];
}

export interface ExperimentDefinition {
  experiment: string;
  hypothesis: string;
  owner: string;
  for_each: string;
  assign_by: string | null;
  /** Absent on a design that is only analysed from past data: nothing is switched. */
  lever?: {
    on: LeverRef;
    off: LeverRef;
    read_back: { when_on: string; when_off: string; grace_days: number };
    on_close: "off" | "keep";
    per: "unit" | "wave";
  };
  design: {
    timezone: string;
    time_dimension: string | null;
    /** The period each panel cell covers; absent = day. Every `_days` count is in these. */
    grain?: "day" | "week" | "month";
    /** Who chose the treated units; absent = randomised by pricing. `observed` = a past rollout, declared. */
    assignment?: "randomised" | "observed";
    waves: { switch_on: string; units: string[] }[];
    switchback: { period_days: number; periods: { from: string; state: "on" | "off" }[] } | null;
    strata: string[][] | null;
    seed: number | null;
    pre_days: number | null;
    post_days: number | null;
    anticipation_days: number;
    washout_days: number;
    /** Days read after the levers turn off at close; absent = none. */
    carryover_days?: number;
    coverage_floor: number;
    compliance_floor: number;
  };
  outcome: {
    target: string;
    /** Makes the outcome a ratio per unit: Σ target ÷ Σ denominator. Read from past data only. */
    denominator?: string;
    /** Absent when nothing narrows the target (the server omits an empty list). */
    target_filter?: MeasureFilter[];
    driver: string | null;
    driver_filter?: MeasureFilter[];
    family: number;
    direction: "increase" | "decrease";
    alpha: number;
    power: number;
    mde_at_proposal: number | null;
    prior_coefficient: { value: number; source: "declared" | "fitted" } | null;
  };
  substitution_check: { sibling: string; filter?: MeasureFilter[] } | null;
  guardrails: {
    criteria:
      | { type: "anomaly"; measure: string; significance: number }
      | { type: "trigger"; when: string };
    action: "abort";
  }[];
  [extra: string]: unknown;
}

/**
 * A design as it is written (create, or a draft save): only what the author
 * set, so a key left out takes the server's default. The server deserializes
 * it into the same `ExperimentConfig` a stored design decodes to.
 */
export type ExperimentDesignInput = Partial<ExperimentDefinition> &
  Pick<ExperimentDefinition, "experiment">;

/** One item per experiment record. */
export interface ExperimentListItem {
  name: string;
  owner: string;
  hypothesis: string;
  /** The experiment's one design. */
  design_id: string;
  design_hash: string;
  /** Set once registered: the design is frozen and never changes. */
  design_frozen_at: string | null;
  status: ExperimentStatus;
  registration_refusal: string | null;
  next_switch_on: string | null;
  /** The run was created from past data, not pre-registered. */
  retrospective: boolean;
  /** No run yet, and every date of the design is past: it can be analysed retrospectively. */
  retrospective_ready: boolean;
  proposals_count: number;
  latest_proposal: ProposalSummary | null;
  /** The whole compiled definition; rendered by the Definition section. */
  definition: ExperimentDefinition;
}

// ---- GET /run (PR 4 Task 13), POST /abort ------------------------------------

/** `off` turns every treated unit's lever off; `keep` leaves each as it is (a guardrail abort is always `off`). */
export type AbortLevers = "off" | "keep";

export interface AbortRequest {
  reason: string;
  levers: AbortLevers;
}

export type GuardrailState = "ok" | "breached" | "not_checked" | "refused";

/**
 * Per-status flip counts. `sent` is NOT exclusive: it counts every flip that
 * went out (has a task), so it overlaps `confirmed`, `unconfirmed` and `failed`.
 * Dispatched but not yet settled = `sent - confirmed - unconfirmed - failed`.
 * `pending` and `cancelled` are exclusive status buckets.
 */
export interface FlipCounts {
  pending: number;
  sent: number;
  confirmed: number;
  unconfirmed: number;
  failed: number;
  cancelled: number;
}

export interface FailedFlip {
  unit: string;
  /** The flip's own state string from the server (a free string on the wire). */
  state: string;
  reason: string;
}

/** One unit's standing against one guardrail. Under `assign_by` the unit is a group. */
export interface UnitGuardrail {
  unit: string;
  status: GuardrailState;
  detail: string | null;
}

export interface GuardrailStatus {
  criteria: string;
  /** The worst unit's status: one breach is a breach. */
  status: GuardrailState;
  detail: string | null;
  /** Every unit evaluated; a breach names the unit here. */
  units: UnitGuardrail[];
}

export type DesignKind = "waves" | "switchback";

/** One switchback period and how far its flips have got (flips are per unit). */
export interface RunPeriod {
  /** The period's 0-based position, as C5 puts it on the wire (equal to the flip `wave`). */
  index: number;
  from: string;
  state: "on" | "off";
  flips: FlipCounts;
}

/** Mirrors the server's `MAX_ESTIMATE_ATTEMPTS`: a failed estimate is run again until this many runs are spent. */
export const MAX_ESTIMATE_ATTEMPTS = 3;

/** The estimate job's standing (`jobs::EstimateState`, tagged by `state`). */
export type EstimateState =
  | { state: "none" }
  | { state: "running" }
  | { state: "done" }
  | {
      state: "failed";
      task_status: string;
      error: string | null;
      failed_at: string;
      attempts: number;
    };

export interface RunDetail {
  state: RunState;
  /** `waves` or `switchback`: decides whether `periods` is set. */
  design: DesignKind;
  /** A switchback's periods in order, consecutive pairs of one on and one off; null for waves. */
  periods: RunPeriod[] | null;
  /** The dimension randomised, when the design assigns groups rather than single units. */
  assign_by: string | null;
  /** group -> members, pinned by the first check; null until then (and without `assign_by`). */
  group_membership: Record<string, string[]> | null;
  /** The frozen design's id. */
  design_id: string;
  /** Who registered it (user id). */
  registered_by: string;
  registered_at: string;
  /** Created from past data: not pre-registered, and no lever is moved. */
  retrospective: boolean;
  next_switch_on: string | null;
  flips: FlipCounts;
  failed_flips: FailedFlip[];
  /** Null until at least one treated flip is decided. */
  compliance: {
    c_treated: number;
    c_control_leak: number;
    restated_mde: number | null;
    below_floor: boolean;
    /** Entries on the treated / control side; `n_treated === 0` means not measured. */
    n_treated: number;
    n_control: number;
  } | null;
  coverage: { last_window: number | null; floor: number };
  guardrails: GuardrailStatus[];
  guardrail_breach: string | null;
  abort_reason: string | null;
  /** What the abort does with the levers: the owner's choice while requested, what it did once aborted. Null until one is requested. */
  abort_levers: AbortLevers | null;
  /** The day the run became aborted (design timezone); its report's measurement ends there. */
  aborted_on: string | null;
  /** Units whose lever is on now: what an abort that turns levers off switches. */
  units_on: string[];
  checked_at: string | null;
  /** What the last check could not evaluate, and why. */
  problems: string[];
  next_check_at: string;
  not_advancing: boolean;
  /** The estimate job: none | running | done | failed. */
  estimate: EstimateState;
}

export interface RunStatus {
  name: string;
  status: ExperimentStatus;
  registration_refusal: string | null;
  retrospective_ready: boolean;
  retrospective_refusal: string | null;
  /** Distinct promoted revisions the flips' levers resolved at, in recording order; more than one = the lever changed mid-run. `[]` until a flip is sent. */
  lever_revisions: string[];
  /** Null until registered. */
  run: RunDetail | null;
}

// ---- GET /result (PR 5 Task 8) ------------------------------------------------

export type Interval = { low: number | null; high: number | null };
export type Direction = "increase" | "decrease";

export type ReportDesign = "units" | "groups" | "switchback";

/** Audit only: the seeds each random step used. */
export interface Seeds {
  effect: number;
  placebo: number;
  ratio: number;
}

/**
 * The World Model's prediction against the measurement (PR 5 Task 8, verbatim).
 * `predicted = prior x ITT(driver)`, the first stage the experiment MEASURED;
 * `measured = ITT(target)`. Rendered from `statement`, never recomposed.
 */
export interface Prediction {
  predicted: number;
  measured: number;
  measured_interval: Interval;
  prior: PriorCoefficient;
  /** Inside the earned (Anderson–Rubin) set? null = the set is unbounded, or a side of it is. */
  prior_in_interval: boolean | null;
  /** "the experiment agrees with / contradicts the World Model's slope …" */
  statement: string;
}

export interface EffectSummary {
  estimate: number | null;
  interval: Interval;
  significant: boolean;
  refusal: string | null;
}

/** What the World Model held when pricing froze it, and the edge's own coefficient at the registering revision. */
export interface ReplacedPrior {
  value: number;
  source: PriorSource;
  edge_coefficient: number | null;
}

export interface CoefficientEdit {
  file_path: string;
  target_measure: string;
  driver_measure: string;
  coefficient: number;
  refs_entry: string;
  /** The prior this edit would replace; null when no prior was frozen. */
  replaces: ReplacedPrior | null;
}

/** The report, in the server's own words wherever a rule is worded. */
export interface ReportView {
  status:
    | { kind: "registered" }
    | { kind: "aborted"; reason: string | null; levers_kept?: boolean }
    | { kind: "retrospective" }
    /** A reading over the days finished so far of a live / closing run; never the result. */
    | { kind: "interim"; through: string };
  /** "Registered result" | "ABORTED RUN — not the registered result (…)". */
  label: string;
  experiment: string;
  hypothesis: string;
  target: string;
  unit: string;
  /** `design.grain`: every per-unit figure is per unit-period of it. Absent on an older report = day. */
  grain?: "day" | "week" | "month";
  /** The unit of the estimate: stores, groups of stores ($/group-day), or the whole fleet (a switchback). */
  design: ReportDesign;
  /** "intention-to-treat effect per unit-day" | "… per group-day" | the switchback's short-run estimand. */
  estimand: string;
  /** Set => render this and NO numbers. */
  refusal: string | null;
  /** The registered α is above 0.05: a result that clears it is directional, never significant. */
  directional: boolean;
  /** The registered α. */
  alpha: number;
  /** What the design cannot identify (a switchback's carryover and pull-forward); render every one, every time. */
  assumptions: string[];
  headline: {
    estimate: number;
    half_width: number | null;
    interval: Interval;
    p_value: number | null;
    /** The engine's decision. Render `verdict`, not this. */
    significant: boolean;
    /** "significant at α = 0.05" | "directional at α = 0.2" | "not distinguishable from zero at α = …". */
    verdict: string;
    text: string;
  } | null;
  treated_total: {
    units: number;
    days: number;
    total: number;
    interval: Interval;
    basis: string;
  } | null;
  projection: {
    label: string;
    untreated_units: number;
    days: number;
    total: number;
    assumption: string;
    warning: string | null;
  } | null;
  /** The effect after the levers turned off; absent or null when the design reads none. */
  carryover?: {
    days: number;
    effect: EffectSummary;
    lingered: boolean;
    statement: string;
  } | null;
  substitution: {
    sibling: string | null;
    floor: number | null;
    statement: string;
    sibling_effect: EffectSummary | null;
    /** The sibling moved significantly against the target: substitution, not a floor. */
    detected: boolean;
  };
  direction: {
    predicted: Direction;
    observed: Direction | "none";
    mismatch: boolean;
    message: string | null;
  };
  promise: {
    mde_at_proposal: number;
    abs_estimate: number;
    below_promise: boolean;
    message: string;
  } | null;
  compliance: {
    c_treated: number;
    c_control_leak: number;
    /** Entries read for treated units; 0 means compliance was not measured (c_treated is then not a share). */
    n_treated: number;
    /** Entries read for control units. */
    n_control: number;
    /** Deliberately never rendered. */
    net: number;
    floor: number;
    below_floor: boolean;
    restated_mde: number | null;
    warning: string | null;
    restated: string | null;
    /** PR 5's own sentence, set exactly when n_treated is 0. Optional here so existing fixtures type-check;
     *  the page decides on n_treated (complianceLabel) and does not need it. */
    not_measured?: string | null;
  };
  checks: {
    pre_trend: { estimate: number | null; t_stat: number | null; diverged: boolean } | null;
    pre_trend_note: string;
    dropped_waves: string[];
    /** `unit: reason` for each unit the estimate left out (missing or sparse data). */
    dropped_units?: string[];
  };
  proposals: {
    count: number;
    message: string;
    /** The split was declared (`design.assignment: observed`), not drawn. */
    observational?: boolean;
    /** Proposals were logged, but the analysed split is none of them. */
    hand_assigned?: boolean;
  };
  /** Why no smallest detectable effect is stated: none was priced (server-worded). */
  mde_note?: string | null;
  /** The outcome is a ratio per unit: its numbers are a share, not money per unit-day. */
  share?: boolean;
  ratio: {
    coefficient: number | null;
    interval: Interval;
    unbounded: boolean;
    first_stage: EffectSummary;
    refusal: string | null;
  } | null;
  /** The World Model's prediction against the measurement; null when no slope was frozen or it could not be compared. */
  prediction: Prediction | null;
  /** Why a frozen prior could not be compared (no first stage, refused, aborted); server-worded. */
  prediction_note: string | null;
  write_back:
    | {
        status: "proposed";
        coefficient: number;
        target: string;
        driver: string;
        file_path: string;
        refs_entry: string;
        replaces: ReplacedPrior | null;
      }
    | { status: "refused"; reason: string }
    | null;
  /** Set when the lever's automation ran more than one distinct revision; server-worded. */
  lever_note: string | null;
  /** Audit only — never rendered. */
  seeds: Seeds;
}

/** A wave's switch date and its registered windows; every `*_to` is exclusive. */
export interface SeriesWave {
  switch_on: string;
  units: number;
  pre_from: string;
  pre_to: string;
  post_from: string;
  post_to: string;
}

/** One retained day of a waves design: unweighted arm means; `null` when the arm holds no unit. */
export interface SeriesWaveDay {
  date: string;
  treated: number | null;
  control: number | null;
  on_units: number;
}

/** One switchback period; `to` is exclusive. */
export interface SeriesPeriod {
  from: string;
  to: string;
  on: boolean;
}

export interface SeriesSwitchbackDay {
  date: string;
  fleet: number;
  on: boolean;
  washout: boolean;
}

/**
 * The target measure day by day across the whole run, sealed with the result from the
 * panel the estimate read. Days the panel dropped are absent. `measured_until` is an
 * aborted run's exclusive cut.
 */
export type DailySeries = {
  measure: string;
  /** Each point covers one of these; absent on a result sealed before grains = day. */
  grain?: "day" | "week" | "month";
  measured_until: string | null;
} & (
  | {
      design: "waves";
      treated_units: number;
      control_units: number;
      waves: SeriesWave[];
      days: SeriesWaveDay[];
    }
  | {
      design: "switchback";
      fleet_units: number;
      washout_days: number;
      periods: SeriesPeriod[];
      days: SeriesSwitchbackDay[];
    }
);

export interface ResultResponse {
  experiment: string;
  run_id: string;
  state: "reported" | "aborted";
  design_id: string;
  aborted: boolean;
  created_at: string;
  report: ReportView;
  /** Audit only — never rendered. */
  effect: Record<string, unknown>;
  /** Audit only — never rendered. */
  ratio: Record<string, unknown> | null;
  compliance: {
    c_treated: number;
    c_control_leak: number;
    n_treated: number;
    n_control: number;
    restated_mde: number | null;
    below_floor: boolean;
  };
  write_back: { available: boolean; reason: string | null; edit: CoefficientEdit | null };
  /** Lifted from `report`, equal to it by construction. The UI reads `report`; these are not read. */
  directional: boolean;
  alpha: number;
  prediction: Prediction | null;
  assumptions: string[];
  /** `null` on a result sealed before the series was recorded, or when the target panel was unavailable. */
  series?: DailySeries | null;
}

/** The two 409s are values, not errors. */
export type ResultOutcome =
  | { kind: "report"; result: ResultResponse }
  | { kind: "sealed"; state: string }
  | { kind: "pending"; state: "reported" | "aborted" };

/** One interim reading: the registered estimator over the days finished so far. */
export interface InterimReading {
  computed_at: string;
  /** `YYYY-MM-DD`, the last day read, inclusive. */
  through: string;
  /** `report.status` is `{ kind: "interim" }`; totals, projection, write-back and prediction are null. */
  report: ReportView;
  /** Audit only — never rendered. */
  effect: unknown;
  compliance: ResultResponse["compliance"];
  series: DailySeries | null;
  alpha: number | null;
  directional: boolean | null;
}

/** `GET` / `POST /experiments/{name}/interim`. */
export interface InterimResponse {
  experiment: string;
  run_id: string;
  state: string;
  design_id: string;
  /** A reading is being computed now. */
  running: boolean;
  requested_at: string | null;
  /** Why the last attempt produced no reading; a previous reading, if any, is still shown. */
  error: string | null;
  /** Null until the first reading finishes. */
  reading: InterimReading | null;
}

/** The 409 is a value, not an error: the run is not live / closing. */
export type InterimOutcome =
  | { kind: "interim"; interim: InterimResponse }
  | { kind: "not_in_progress"; state: string; error: string | null };

/** `POST /experiments/{name}/write-back/apply`. */
export interface WriteBackResponse {
  branch: string;
  file_path: string;
}

// ---- The design (one editable draft until registered) -----------------------

/** An experiment's one design; `config` is the full `ExperimentConfig` JSON. Frozen once `frozen_at` is set. */
export interface Design {
  id: string;
  content_hash: string;
  frozen_at: string | null;
  /** Frozen, but its only run is a refused analysis of past data: an edit reopens it. */
  reopenable?: boolean;
  from_proposal_id: string | null;
  created_by: string;
  created_at: string;
  updated_by: string;
  updated_at: string;
  config: ExperimentDefinition;
}

/** `POST /experiments/{name}/register`. */
export interface RegisterResponse {
  run_id: string;
  design_id: string;
  registered_at: string;
  frozen_at: string;
}
