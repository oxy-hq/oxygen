import type { MetricNode } from "@/types/metricTree";
import type { WorldModelEntity } from "@/types/worldModel";
import type { Choice } from "./components/fields";
import type { Match, PickerGroup } from "./components/PickerField";

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

// ---- measures ---------------------------------------------------------------

/** Measures the platform injects (such as `view.__oxy_row_count`) are not offered. */
const isInjected = (n: MetricNode) => n.measure.startsWith("__");

/** The semantic model's measures (`view.measure`), grouped by view. */
export function measureGroups(nodes: MetricNode[]): PickerGroup[] {
  const views = new Map<string, MetricNode[]>();
  for (const n of nodes.filter((x) => !isInjected(x)))
    views.set(n.view, [...(views.get(n.view) ?? []), n]);
  return [...views.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([view, ns]) => ({
      title: view,
      options: [...ns]
        .sort((a, b) => a.id.localeCompare(b.id))
        .map((n) => ({
          value: n.id,
          label: n.id,
          detail: n.label && n.label !== n.measure ? n.label : (n.description ?? ""),
          search: `${n.id} ${n.label} ${n.description ?? ""}`
        }))
    }));
}

export function measureMatch(value: string, nodes: MetricNode[] | undefined): Match | null {
  const v = value.trim();
  if (!v || !nodes) return null;
  const n = nodes.find((x) => x.id === v);
  return n
    ? { tone: "success", text: `Measure ${n.label && n.label !== n.measure ? n.label : n.id}` }
    : { tone: "warning", text: "Not a measure in the semantic model" };
}

// ---- entities and dimensions --------------------------------------------------

export function entityGroups(entities: WorldModelEntity[]): PickerGroup[] {
  return [
    {
      options: [...entities]
        .sort((a, b) => a.id.localeCompare(b.id))
        .map((e) => ({
          value: e.id,
          label: e.id,
          detail: e.description || `view ${e.view}`,
          search: `${e.id} ${e.view} ${e.description ?? ""}`
        }))
    }
  ];
}

export function entityMatch(value: string, entities: WorldModelEntity[] | undefined): Match | null {
  const v = value.trim();
  if (!v || !entities) return null;
  const e = entities.find((x) => x.id === v);
  return e
    ? { tone: "success", text: `Entity of view ${e.view}` }
    : { tone: "warning", text: "Not an entity in the semantic model" };
}

export function dimensionGroups(
  entityId: string,
  entities: WorldModelEntity[] | undefined
): PickerGroup[] {
  const e = entities?.find((x) => x.id === entityId.trim());
  if (!e) return [];
  return [
    {
      options: [...e.dimensions].sort(byName).map((d) => ({
        value: d.name,
        label: d.name,
        detail: d.label && d.label !== d.name ? d.label : d.dim_type,
        search: `${d.name} ${d.label ?? ""} ${d.description ?? ""}`
      }))
    }
  ];
}

/** The server takes `dimension` or `view.dimension` (crates/app/src/server/experiment/panel_groups.rs). */
export function dimensionMatch(
  value: string,
  entityId: string,
  entities: WorldModelEntity[] | undefined
): Match | null {
  const v = value.trim();
  const e = entities?.find((x) => x.id === entityId.trim());
  if (!v || !e) return null;
  const [first, rest] = v.split(/\.(.*)/s);
  const name = rest !== undefined && first === e.view ? rest : v;
  return e.dimensions.some((d) => d.name === name)
    ? { tone: "success", text: `Dimension of ${e.id}` }
    : { tone: "warning", text: `Not a dimension of ${e.id}` };
}

// ---- time dimensions ------------------------------------------------------------

/** The outcome's view when it is known: the server takes only one of that view's dates. */
function timeDimensionScope(target: string, byView: Record<string, string[]>) {
  const view = target.trim().split(".")[0];
  return view && byView[view] ? ([view] as string[]) : Object.keys(byView).sort();
}

export function timeDimensionGroups(
  target: string,
  byView: Record<string, string[]>
): PickerGroup[] {
  return timeDimensionScope(target, byView)
    .filter((v) => (byView[v] ?? []).length > 0)
    .map((view) => ({
      title: view,
      options: [...(byView[view] ?? [])].sort().map((id) => ({ value: id, label: id }))
    }));
}

export function timeDimensionMatch(
  value: string,
  target: string,
  byView: Record<string, string[]> | undefined
): Match | null {
  const v = value.trim();
  if (!v || !byView) return null;
  const scope = timeDimensionScope(target, byView);
  if (scope.some((view) => (byView[view] ?? []).includes(v))) {
    return { tone: "success", text: "Date dimension" };
  }
  return {
    tone: "warning",
    text:
      scope.length === 1
        ? `Not a date dimension of ${scope[0]}, the outcome's view`
        : "Not a date dimension in the semantic model"
  };
}

// ---- automations ----------------------------------------------------------------

interface AutomationLike {
  path: string;
  description?: string | null;
}

/**
 * The name a lever's `via` resolves against: the file's basename without its
 * extension (`procedures/fruit_summary.automation.yml` -> `fruit_summary`).
 */
const automationName = (a: AutomationLike) =>
  (a.path.split("/").pop() ?? a.path).replace(/\.(automation|workflow|procedure)\.ya?ml$/i, "");

export function automationGroups(automations: AutomationLike[]): PickerGroup[] {
  const seen = new Set<string>();
  const options = automations
    .map((a) => ({ value: automationName(a), label: automationName(a), detail: a.path }))
    .sort((a, b) => a.value.localeCompare(b.value))
    .filter((o) => !seen.has(o.value) && seen.add(o.value));
  return [{ options }];
}

/** A lever must resolve to exactly one automation by name (crates/app/src/server/experiment/lever.rs). */
export function automationMatch(
  value: string,
  automations: AutomationLike[] | undefined
): Match | null {
  const v = value.trim();
  if (!v || !automations) return null;
  const n = automations.filter((a) => automationName(a) === v).length;
  if (n === 1) return { tone: "success", text: "Automation" };
  if (n === 0) return { tone: "warning", text: "No automation has this name" };
  return { tone: "warning", text: `${n} automations have this name: it must be one` };
}

// ---- selects ----------------------------------------------------------------------

/** The standard choices, plus a stored value that is none of them so the select still shows it. */
export function withCurrent<T extends string>(choices: Choice<T>[], current: string): Choice<T>[] {
  if (current === "" || choices.some((c) => c.value === current)) return choices;
  return [...choices, { value: current as T, label: `${current} (custom)` }];
}

// ---- time zones -----------------------------------------------------------------------

const FALLBACK_ZONES = [
  "America/New_York",
  "America/Chicago",
  "America/Los_Angeles",
  "Europe/London",
  "Europe/Berlin",
  "Asia/Ho_Chi_Minh",
  "Asia/Singapore",
  "Australia/Sydney"
];

/** UTC, then the IANA zones the runtime knows (the server takes any chrono-tz name). */
export function timeZones(): string[] {
  let zones: string[];
  try {
    zones = Intl.supportedValuesOf("timeZone");
  } catch {
    zones = FALLBACK_ZONES;
  }
  return ["UTC", ...zones.filter((z) => z !== "UTC")];
}
