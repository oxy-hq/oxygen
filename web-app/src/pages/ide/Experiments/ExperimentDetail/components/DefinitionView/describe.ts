import { type Grain, periodWord } from "@/pages/ide/Experiments/grain";
import type { ExperimentDefinition, MeasureFilter } from "@/types/experiment";

type Design = ExperimentDefinition["design"];
type Guardrail = ExperimentDefinition["guardrails"][number];

/** `vendor_note` -> `Vendor note`. */
export const humanize = (key: string): string => {
  const s = key.replace(/_+/g, " ").trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
};

/** `3-day`, `2-week`: a length as an adjective. */
const lasting = (n: number, grain: Grain = "day") => `${n}-${grain}`;

/** `shopper_days.order_value` -> `Order value per shopper-day`; other views: just the field. */
export function measureLabel(ref: string): string {
  const [view, field] = ref.includes(".") ? ref.split(/\.(.+)/) : ["", ref];
  const label = humanize(field ?? ref);
  return view?.endsWith("_days") ? `${label} per ${view.slice(0, -1).replace(/_/g, "-")}` : label;
}

/** Values joined with "or"; a long list names its first three and counts the rest. */
export function valueList(values: string[]): string {
  if (values.length <= 5) return values.join(" or ");
  return `${values.slice(0, 3).join(", ")} or ${values.length - 3} more`;
}

/** A measure's filters as words ("only rows where sku is TB-40"); empty when there are none. */
export function filterText(filters: MeasureFilter[] | undefined): string {
  if (!filters || filters.length === 0) return "";
  const clauses = filters.map((f) => {
    const dim = f.dimension.includes(".") ? f.dimension.split(/\.(.+)/)[1] : f.dimension;
    return `${dim} ${f.op === "equals" ? "is" : "is not"} ${valueList(f.values)}`;
  });
  return `only rows where ${clauses.join(" and ")}`;
}

export const randomisesText = (forEach: string, assignBy: string | null) =>
  assignBy ? `Randomises groups of ${forEach} by ${assignBy}` : `Randomises each ${forEach}`;

export const directionText = (d: "increase" | "decrease") =>
  d === "increase" ? "expects an increase" : "expects a decrease";

/** α up to 0.05 is a plain test; above it the experiment was registered as directional. */
export const significanceText = (alpha: number) => {
  const a = String(Number(alpha.toPrecision(3)));
  return alpha > 0.05 ? `Directional, α ${a}` : `α ${a}`;
};

export const percent = (share: number) => `${Number((share * 100).toFixed(1))}%`;

export const closeText = (c: "off" | "keep") =>
  c === "off" ? "Turned off when the experiment closes" : "Left on when the experiment closes";

export const perText = (p: "unit" | "wave") =>
  p === "wave"
    ? "One automation run for each wave and state, carrying all of the wave's units"
    : "One automation run for each unit and each switch";

export const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;

/** The pre/post windows and the buffers either side of each switch, as one sentence. */
export function windowsText(
  d: Pick<Design, "pre_days" | "post_days" | "anticipation_days" | "washout_days">,
  grain: Grain = "day"
) {
  const parts: string[] = [];
  const many = periodWord(grain, { plural: true });
  if (d.pre_days !== null && d.post_days !== null)
    parts.push(`${d.pre_days} ${many} before and ${d.post_days} ${many} after each switch`);
  parts.push(
    d.anticipation_days > 0
      ? `${lasting(d.anticipation_days, grain)} anticipation buffer`
      : "no anticipation buffer"
  );
  parts.push(d.washout_days > 0 ? `${lasting(d.washout_days, grain)} washout` : "no washout");
  const s = parts.join("; ");
  return `${s.charAt(0).toUpperCase()}${s.slice(1)}.`;
}

export function guardrailText(g: Guardrail): string {
  const c = g.criteria;
  return c.type === "anomaly"
    ? `Aborts the run if ${measureLabel(c.measure)} looks anomalous (significance ${c.significance}).`
    : "Aborts the run when:";
}

export const periodsText = (s: NonNullable<Design["switchback"]>, grain: Grain = "day") =>
  s.periods.length === 0
    ? `Switchback in ${lasting(s.period_days, grain)} periods — the schedule is not priced yet.`
    : `The whole fleet alternates in ${lasting(s.period_days, grain)} periods: ${plural(s.periods.length / 2, "on/off pair")}, starting ${s.periods[0].state} on ${s.periods[0].from}.`;

/** Alpha as the confidence it buys: 0.05 -> `95% confidence`. */
export const confidenceText = (alpha: number) => `${percent(1 - alpha)} confidence`;
