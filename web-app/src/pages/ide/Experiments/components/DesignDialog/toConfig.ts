import { isCalendarDate } from "@/pages/ide/Experiments/dates";
import {
  type Grain,
  isSwitchMoment,
  periodStartRule,
  periodWord,
  switchDate
} from "@/pages/ide/Experiments/grain";
import type { ExperimentDefinition, ExperimentDesignInput } from "@/types/experiment";
import { DEFAULTS, type DesignFormValues, type FilterRow, type GuardrailRow } from "./designForm";

/** Messages keyed by form path (`graceDays`, `waves.0.switchOn`, …). */
export type DesignErrors = Record<string, string>;

/**
 * The config the server takes. Only what the person set is sent: a value left
 * at the server's default, or an optional one left empty, is omitted, so the
 * object is exactly what the design file would have said.
 */
export type DesignConfig = ExperimentDesignInput;

/** Seeds are drawn below 2^53, so up to 16 digits; anything past a safe integer is refused. */
const WHOLE = /^\d{1,16}$/;
const DECIMAL = /^-?(\d+\.?\d*|\.\d+)(e-?\d+)?$/i;

const NAME = /^[A-Za-z0-9_]+$/;

/** Why a new experiment's name will not do, if it will not. */
export const nameProblem = (name: string): string | undefined =>
  NAME.test(name) ? undefined : "Use letters, digits and underscores only, with no spaces.";

const unitNames = (raw: string) =>
  raw
    .split(/[\n,]/)
    .map((u) => u.trim())
    .filter((u) => u !== "");

/** The form fields whose value is free text. */
type TextKey = {
  [K in keyof DesignFormValues]: DesignFormValues[K] extends string ? K : never;
}[keyof DesignFormValues];

const fromPercent = (p: number) => Number((p / 100).toPrecision(12));

/** One form's conversion: the errors it finds and the config it builds as it goes. */
class Reader {
  errors: DesignErrors = {};
  constructor(private v: DesignFormValues) {}

  required(name: TextKey, message: string): string {
    const s = this.v[name].trim();
    if (s === "") this.errors[name] = message;
    return s;
  }

  optional(name: TextKey): string | undefined {
    const s = this.v[name].trim();
    return s === "" ? undefined : s;
  }

  whole(path: string, raw: string, opts: { min?: number; required?: string } = {}) {
    const s = raw.trim();
    if (s === "") {
      if (opts.required) this.errors[path] = opts.required;
      return undefined;
    }
    const min = opts.min ?? 0;
    if (!WHOLE.test(s) || !Number.isSafeInteger(Number(s)) || Number(s) < min) {
      this.errors[path] = `Enter a whole number of ${min} or more.`;
      return undefined;
    }
    return Number(s);
  }

  decimal(path: string, raw: string, check?: (n: number) => string | null) {
    const s = raw.trim();
    if (s === "") return undefined;
    const n = Number(s);
    if (!DECIMAL.test(s) || !Number.isFinite(n)) {
      this.errors[path] = "Enter a number, for example 0.05.";
      return undefined;
    }
    const problem = check?.(n) ?? null;
    if (problem) this.errors[path] = problem;
    return problem ? undefined : n;
  }

  date(path: string, raw: string, grain: Grain = "day") {
    const s = raw.trim();
    if (!isCalendarDate(switchDate(s))) this.errors[path] = "Enter a calendar date.";
    else if (!isSwitchMoment(s, grain)) this.errors[path] = periodStartRule(grain);
    return s;
  }
}

const between = (lo: number, hi: number, inclusiveHi: boolean, what: string) => (n: number) =>
  n > lo && (inclusiveHi ? n <= hi : n < hi) ? null : what;

function guardrail(r: Reader, g: GuardrailRow, i: number) {
  const at = `guardrails.${i}`;
  if (g.kind === "trigger") {
    if (g.when.trim() === "") r.errors[`${at}.when`] = "Enter the condition that stops the run.";
    return {
      criteria: { type: "trigger" as const, when: g.when.trim() },
      action: "abort" as const
    };
  }
  if (g.measure.trim() === "") r.errors[`${at}.measure`] = "Enter the measure to watch.";
  const significance = r.decimal(
    `${at}.significance`,
    g.significance,
    between(0, 1, false, "Significance must be above 0 and below 1.")
  );
  if (g.significance.trim() === "") r.errors[`${at}.significance`] = "Enter a significance level.";
  return {
    criteria: {
      type: "anomaly" as const,
      measure: g.measure.trim(),
      significance: significance ?? 0
    },
    action: "abort" as const
  };
}

function lever(r: Reader, v: DesignFormValues) {
  const out: Record<string, unknown> = {
    on: { via: r.required("onVia", "Name the automation that turns the change on.") },
    off: { via: r.required("offVia", "Name the automation that turns the change off.") },
    read_back: {
      when_on: r.required("whenOn", "Enter the condition that shows the change is on."),
      when_off: r.required("whenOff", "Enter the condition that shows the change is off."),
      grace_days: r.whole("graceDays", v.graceDays, { required: "Enter a number of days." })
    }
  };
  if (v.onClose !== "off") out.on_close = v.onClose;
  if (v.per !== "unit") out.per = v.per;
  return out as ExperimentDefinition["lever"];
}

function schedule(r: Reader, v: DesignFormValues) {
  const out: Record<string, unknown> = {
    timezone: r.required("timezone", "Enter a time zone, for example UTC.")
  };
  const timeDimension = r.optional("timeDimension");
  if (timeDimension) out.time_dimension = timeDimension;
  if (v.grain !== "day") out.grain = v.grain;
  if (v.kind === "waves") {
    if (v.waves.length > 0) {
      out.waves = v.waves.map((w, i) => {
        const units = unitNames(w.units);
        if (units.length === 0) r.errors[`waves.${i}.units`] = "List at least one name.";
        return { switch_on: r.date(`waves.${i}.switchOn`, w.switchOn, v.grain), units };
      });
    }
    if (v.strata.length > 0) {
      out.strata = v.strata.map((s, i) => {
        const units = unitNames(s.units);
        if (units.length === 0) r.errors[`strata.${i}.units`] = "List at least one name.";
        return units;
      });
    }
    const pre = r.whole("preDays", v.preDays, { min: 1 });
    const post = r.whole("postDays", v.postDays, { min: 1 });
    const anticipation = r.whole("anticipationDays", v.anticipationDays);
    if (pre !== undefined) out.pre_days = pre;
    if (post !== undefined) out.post_days = post;
    if (anticipation) out.anticipation_days = anticipation;
    const carryover = r.whole("carryoverDays", v.carryoverDays, { min: 1 });
    if (carryover !== undefined) out.carryover_days = carryover;
  } else {
    const periods = v.periods.map((p, i) => ({
      from: r.date(`periods.${i}.from`, p.from, v.grain),
      state: p.state
    }));
    const periodDays = r.whole("periodDays", v.periodDays, {
      min: 1,
      required: `Enter a number of ${periodWord(v.grain, { plural: true })}.`
    });
    out.switchback = {
      period_days: periodDays,
      ...(periods.length > 0 ? { periods } : {})
    };
  }
  const seed = r.whole("seed", v.seed);
  if (seed !== undefined) out.seed = seed;
  const washout = r.whole("washoutDays", v.washoutDays);
  if (washout) out.washout_days = washout;
  floors(r, v, out);
  return out as ExperimentDefinition["design"];
}

function floors(r: Reader, v: DesignFormValues, out: Record<string, unknown>) {
  const pct = between(0, 100, true, "Enter a percentage above 0 and up to 100.");
  const coverage = r.decimal("coverageFloor", v.coverageFloor, pct);
  const compliance = r.decimal("complianceFloor", v.complianceFloor, pct);
  if (coverage !== undefined && fromPercent(coverage) !== DEFAULTS.coverageFloor)
    out.coverage_floor = fromPercent(coverage);
  if (compliance !== undefined && fromPercent(compliance) !== DEFAULTS.complianceFloor)
    out.compliance_floor = fromPercent(compliance);
}

type FilterList = "targetFilter" | "driverFilter" | "siblingFilter";

/** A measure's filter rows as the config's list; `undefined` when there are none. */
function filters(r: Reader, rows: FilterRow[], name: FilterList) {
  if (rows.length === 0) return undefined;
  return rows.map((f, i) => {
    if (f.dimension.trim() === "")
      r.errors[`${name}.${i}.dimension`] = "Choose what to narrow the measure by.";
    if (f.values.length === 0) r.errors[`${name}.${i}.values`] = "Choose at least one value.";
    return { dimension: f.dimension.trim(), op: f.op, values: f.values };
  });
}

function outcome(r: Reader, v: DesignFormValues) {
  const out: Record<string, unknown> = {
    target: r.required("target", "Enter the measure the experiment reads."),
    direction: v.direction
  };
  const targetFilter = filters(r, v.targetFilter, "targetFilter");
  if (targetFilter) out.target_filter = targetFilter;
  const denominator = r.optional("denominator");
  if (denominator) out.denominator = denominator;
  if (v.scale === "proportional") out.scale = "proportional";
  const driver = r.optional("driver");
  if (driver) out.driver = driver;
  const driverFilter = filters(r, v.driverFilter, "driverFilter");
  if (driverFilter) out.driver_filter = driverFilter;
  const family = r.whole("family", v.family, { min: 1 });
  if (family !== undefined && family !== DEFAULTS.family) out.family = family;
  const alpha = r.decimal(
    "alpha",
    v.alpha,
    between(0, 1, false, "Enter a level above 0 and below 1.")
  );
  if (alpha !== undefined && alpha !== DEFAULTS.alpha) out.alpha = alpha;
  const power = r.decimal(
    "power",
    v.power,
    between(0, 100, false, "Enter a percentage above 0 and below 100.")
  );
  if (power !== undefined && fromPercent(power) !== DEFAULTS.power) out.power = fromPercent(power);
  const mde = r.decimal("mdeAtProposal", v.mdeAtProposal, (n) =>
    n > 0 ? null : "Must be greater than 0."
  );
  if (mde !== undefined) out.mde_at_proposal = mde;
  const prior = r.decimal("priorValue", v.priorValue);
  if (prior !== undefined) out.prior_coefficient = { value: prior, source: v.priorSource };
  return out as ExperimentDefinition["outcome"];
}

/** The form as the config to post, or the per-field reasons it is not one yet. */
export function toConfig(
  v: DesignFormValues,
  opts: {
    /** Hold the name to the identifier rule: for a new experiment, not for one that already has its name. */
    checkName?: boolean;
  } = {}
): { config: DesignConfig; errors: DesignErrors } {
  const r = new Reader(v);
  const config: DesignConfig = {
    experiment: r.required("experiment", "Give the experiment a name."),
    hypothesis: r.required("hypothesis", "Describe the effect you expect."),
    owner: r.required("owner", "Name who owns the experiment."),
    for_each: r.required("forEach", "Name the entity that is randomised, for example customer.")
  };
  const problem = opts.checkName && config.experiment ? nameProblem(config.experiment) : undefined;
  if (problem) r.errors.experiment = problem;
  const assignBy = r.optional("assignBy");
  if (assignBy) config.assign_by = assignBy;
  // An observed rollout already happened: nothing is switched, so there is no lever.
  if (v.assignment !== "observed") config.lever = lever(r, v);
  config.design = schedule(r, v);
  if (v.assignment === "observed") {
    config.design.assignment = "observed";
    if (v.comparison === "synthetic_control") {
      config.design.comparison = "synthetic_control";
      const exclude = unitNames(v.exclude);
      if (exclude.length > 0) config.design.exclude = exclude;
    }
  }
  // Picks the test for one switch date, randomised or observed; a switchback and a
  // synthetic control have their own test, so it is not written for either.
  const synthetic = v.assignment === "observed" && v.comparison === "synthetic_control";
  if (v.kind === "waves" && !synthetic && v.oneDateTest === "permutation")
    config.design.one_date_test = "permutation";
  config.outcome = outcome(r, v);
  const sibling = r.optional("sibling");
  const siblingFilter = filters(r, v.siblingFilter, "siblingFilter");
  if (sibling)
    config.substitution_check = siblingFilter ? { sibling, filter: siblingFilter } : { sibling };
  if (v.guardrails.length > 0) config.guardrails = v.guardrails.map((g, i) => guardrail(r, g, i));
  return { config, errors: r.errors };
}
