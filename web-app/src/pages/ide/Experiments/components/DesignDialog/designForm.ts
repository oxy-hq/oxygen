import { type Grain, grainOf } from "@/pages/ide/Experiments/grain";
import type { ExperimentDefinition } from "@/types/experiment";

/**
 * The design form's state. Every input is the raw text the person typed (or a
 * fixed choice); `toConfig` turns it into the config the server takes. Field
 * names here are the form's own — the config's keys are never shown.
 */
export type DesignKind = "waves" | "switchback";
export type LeverState = "on" | "off";
export type GuardrailKind = "anomaly" | "trigger";

export interface WaveRow {
  switchOn: string;
  /** Unit (or group) names, separated by commas or new lines. */
  units: string;
}
export interface StratumRow {
  units: string;
}
export interface PeriodRow {
  from: string;
  state: LeverState;
}
/** One filter on a measure: a dimension of its view, and the values kept (or dropped). */
export interface FilterRow {
  dimension: string;
  op: "equals" | "not_equals";
  values: string[];
}
export interface GuardrailRow {
  kind: GuardrailKind;
  measure: string;
  significance: string;
  when: string;
}

export interface DesignFormValues {
  experiment: string;
  hypothesis: string;
  owner: string;
  forEach: string;
  assignBy: string;
  /** `observed`: a rollout that already happened, declared and analysed from past data; no lever. */
  assignment: "randomised" | "observed";
  // lever
  onVia: string;
  offVia: string;
  whenOn: string;
  whenOff: string;
  graceDays: string;
  onClose: "off" | "keep";
  per: "unit" | "wave";
  // schedule
  kind: DesignKind;
  timezone: string;
  timeDimension: string;
  grain: Grain;
  preDays: string;
  postDays: string;
  anticipationDays: string;
  washoutDays: string;
  /** Days read after the lever turns off at close; empty = none. */
  carryoverDays: string;
  periodDays: string;
  /** Percentages, 0–100: the floors read as "at least 90%". */
  coverageFloor: string;
  complianceFloor: string;
  // set by pricing
  waves: WaveRow[];
  strata: StratumRow[];
  periods: PeriodRow[];
  seed: string;
  mdeAtProposal: string;
  priorValue: string;
  priorSource: "declared" | "fitted";
  // outcome
  target: string;
  targetFilter: FilterRow[];
  /** Empty = a plain outcome; a measure = the outcome is target ÷ this, per unit. */
  denominator: string;
  /** How the change is read: in the measure's units, or as a % lift per unit (past data only). */
  scale: "units" | "proportional";
  driver: string;
  driverFilter: FilterRow[];
  direction: "increase" | "decrease";
  family: string;
  alpha: string;
  /** A percentage, 0–100. */
  power: string;
  sibling: string;
  siblingFilter: FilterRow[];
  guardrails: GuardrailRow[];
}

/** The server's defaults (crates/experiment/src/config.rs): a field left at one is not sent. */
export const DEFAULTS = {
  coverageFloor: 0.9,
  complianceFloor: 0.8,
  family: 1,
  alpha: 0.05,
  power: 0.8
} as const;

export const newWave = (): WaveRow => ({ switchOn: "", units: "" });
export const newStratum = (): StratumRow => ({ units: "" });
export const newPeriod = (): PeriodRow => ({ from: "", state: "on" });
export const newFilter = (): FilterRow => ({ dimension: "", op: "equals", values: [] });
export const newGuardrail = (): GuardrailRow => ({
  kind: "anomaly",
  measure: "",
  significance: "0.05",
  when: ""
});

/** A share (0.9) as the percentage text the form shows ("90"), without float noise. */
export const toPercentText = (share: number) => String(Number((share * 100).toPrecision(12)));

export function emptyDesignValues(): DesignFormValues {
  return {
    experiment: "",
    hypothesis: "",
    owner: "",
    forEach: "",
    assignBy: "",
    assignment: "randomised",
    onVia: "",
    offVia: "",
    whenOn: "",
    whenOff: "",
    graceDays: "",
    onClose: "off",
    per: "unit",
    kind: "waves",
    timezone: "UTC",
    timeDimension: "",
    grain: "day",
    preDays: "",
    postDays: "",
    anticipationDays: "0",
    washoutDays: "0",
    carryoverDays: "",
    periodDays: "7",
    coverageFloor: toPercentText(DEFAULTS.coverageFloor),
    complianceFloor: toPercentText(DEFAULTS.complianceFloor),
    waves: [],
    strata: [],
    periods: [],
    seed: "",
    mdeAtProposal: "",
    priorValue: "",
    priorSource: "declared",
    target: "",
    denominator: "",
    scale: "units",
    targetFilter: [],
    driver: "",
    driverFilter: [],
    direction: "increase",
    family: String(DEFAULTS.family),
    alpha: String(DEFAULTS.alpha),
    power: toPercentText(DEFAULTS.power),
    sibling: "",
    siblingFilter: [],
    guardrails: []
  };
}

// ---- config -> form ---------------------------------------------------------

type Loose = Record<string, unknown>;
const obj = (v: unknown): Loose =>
  v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Loose) : {};
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const text = (v: unknown, fallback = ""): string =>
  typeof v === "string" ? v : typeof v === "number" ? String(v) : fallback;
const percent = (v: unknown, fallback: string): string =>
  typeof v === "number" ? toPercentText(v) : fallback;
const names = (v: unknown): string =>
  list(v)
    .map((u) => text(u))
    .join(", ");
const oneOf = <T extends string>(v: unknown, options: readonly T[], fallback: T): T =>
  options.includes(v as T) ? (v as T) : fallback;

function filterRows(v: unknown): FilterRow[] {
  return list(v).map((f) => ({
    dimension: text(obj(f).dimension),
    op: oneOf(obj(f).op, ["equals", "not_equals"] as const, "equals"),
    values: list(obj(f).values).map((x) => text(x))
  }));
}

function guardrailRow(g: unknown): GuardrailRow {
  const c = obj(obj(g).criteria);
  return {
    kind: oneOf(c.type, ["anomaly", "trigger"] as const, "anomaly"),
    measure: text(c.measure),
    significance: text(c.significance),
    when: text(c.when)
  };
}

/**
 * A design (a stored version, or a parsed import that may leave keys out) as
 * form values. Anything absent takes the form's default, which is the server's.
 */
export function toFormValues(input: Partial<ExperimentDefinition> | Loose): DesignFormValues {
  const base = emptyDesignValues();
  const c = obj(input);
  const lever = obj(c.lever);
  const readBack = obj(lever.read_back);
  const design = obj(c.design);
  const switchback = design.switchback ? obj(design.switchback) : null;
  const outcome = obj(c.outcome);
  const prior = outcome.prior_coefficient ? obj(outcome.prior_coefficient) : null;
  return {
    ...base,
    experiment: text(c.experiment),
    hypothesis: text(c.hypothesis),
    owner: text(c.owner),
    forEach: text(c.for_each),
    assignBy: text(c.assign_by),
    assignment: design.assignment === "observed" ? "observed" : "randomised",
    onVia: text(obj(lever.on).via),
    offVia: text(obj(lever.off).via),
    whenOn: text(readBack.when_on),
    whenOff: text(readBack.when_off),
    graceDays: text(readBack.grace_days),
    onClose: oneOf(lever.on_close, ["off", "keep"] as const, base.onClose),
    per: oneOf(lever.per, ["unit", "wave"] as const, base.per),
    kind: switchback ? "switchback" : "waves",
    timezone: text(design.timezone, base.timezone),
    timeDimension: text(design.time_dimension),
    grain: grainOf(design.grain),
    preDays: text(design.pre_days),
    postDays: text(design.post_days),
    anticipationDays: text(design.anticipation_days, base.anticipationDays),
    washoutDays: text(design.washout_days, base.washoutDays),
    carryoverDays: text(design.carryover_days),
    periodDays: switchback ? text(switchback.period_days) : base.periodDays,
    coverageFloor: percent(design.coverage_floor, base.coverageFloor),
    complianceFloor: percent(design.compliance_floor, base.complianceFloor),
    waves: list(design.waves).map((w) => ({
      switchOn: text(obj(w).switch_on),
      units: names(obj(w).units)
    })),
    strata: list(design.strata).map((s) => ({ units: names(s) })),
    periods: list(switchback?.periods).map((p) => ({
      from: text(obj(p).from),
      state: oneOf(obj(p).state, ["on", "off"] as const, "on")
    })),
    seed: text(design.seed),
    mdeAtProposal: text(outcome.mde_at_proposal),
    priorValue: prior ? text(prior.value) : "",
    priorSource: oneOf(prior?.source, ["declared", "fitted"] as const, base.priorSource),
    target: text(outcome.target),
    denominator: text(outcome.denominator),
    scale: oneOf(outcome.scale, ["units", "proportional"] as const, base.scale),
    targetFilter: filterRows(outcome.target_filter),
    driver: text(outcome.driver),
    driverFilter: filterRows(outcome.driver_filter),
    direction: oneOf(outcome.direction, ["increase", "decrease"] as const, base.direction),
    family: text(outcome.family, base.family),
    alpha: text(outcome.alpha, base.alpha),
    power: percent(outcome.power, base.power),
    sibling: text(obj(c.substitution_check).sibling),
    siblingFilter: filterRows(obj(c.substitution_check).filter),
    guardrails: list(c.guardrails).map(guardrailRow)
  };
}
