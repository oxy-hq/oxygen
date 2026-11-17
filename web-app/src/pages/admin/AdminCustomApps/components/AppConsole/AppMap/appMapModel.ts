import type { RecentTraces, TraceTouch } from "@/types/appRecentTraces";
import type { AppFunctionSummary, AppSecretEntry } from "@/types/apps";

/**
 * The app map as data: what starts each function (left), the functions (middle), and
 * what they reach (right) — manifest + recorded traces → nodes and edges. Pure, so the
 * picture and the text list are two renderings of one answer and the rules are tested
 * here rather than read off a screenshot.
 *
 * **Two sources, and the map says which.** The left side and the warehouse
 * destinations are the manifest: what the build *declares*. Everything else on the
 * right is what the latest recorded runs *did* — so a host a function only calls on
 * the first of the month is absent until it has, which is why the panel states the
 * window it drew from (`freshness`).
 */

/** How a function is started. `manual`: nothing in the manifest starts it, so only
 *  Run now (or another function) does. */
export type EntryKind = "route" | "webhook" | "schedule" | "airway" | "check" | "manual";

export interface MapEntry {
  id: string;
  /** The function it starts. */
  fn: string;
  kind: EntryKind;
  label: string;
  /** The path, the cron, the signature header — what tells two of a kind apart. */
  detail: string | null;
}

export interface MapFunction {
  id: string;
  name: string;
  /** Recorded runs drawn from; `null` while the traces have not been read. */
  runs: number | null;
  failedRuns: number;
}

export type TargetGroup =
  | "warehouse"
  | "tables"
  | "oltp"
  | "airhouse"
  | "storage"
  | "fetch"
  | "airway"
  | "semantic"
  | "messaging"
  | "secrets"
  | "platform"
  | "other";

/** Top to bottom on the right: data first, then the outside world, then the app's
 *  own plumbing. */
export const TARGET_GROUPS: readonly TargetGroup[] = [
  "warehouse",
  "tables",
  "oltp",
  "airhouse",
  "storage",
  "fetch",
  "airway",
  "semantic",
  "messaging",
  "secrets",
  "platform",
  "other"
];

export const TARGET_GROUP_LABEL: Record<TargetGroup, string> = {
  warehouse: "Warehouse writes",
  tables: "Tables",
  oltp: "OLTP",
  airhouse: "Airhouse",
  storage: "Storage",
  fetch: "Outbound hosts",
  airway: "Airway pipelines",
  semantic: "Semantic layer",
  messaging: "Email & notify",
  secrets: "Secrets",
  platform: "Org & platform",
  other: "Other"
};

export interface MapTarget {
  id: string;
  group: TargetGroup;
  label: string;
  /** Verbs seen (`SELECT`, `POST`), or a warehouse database's tables. */
  details: string[];
  calls: number;
  /** Calls that ended `error`. The only thing on the map drawn in the danger tone. */
  failedCalls: number;
  /** Named by the manifest (a warehouse destination). */
  declared: boolean;
  /** Called in a recorded run. */
  observed: boolean;
}

export interface MapEdge {
  id: string;
  from: string;
  to: string;
  kind: "trigger" | "touch";
  /** The `ctx.*` families of the calls along it, in first-seen order. */
  families: string[];
  calls: number;
  failedCalls: number;
  declared: boolean;
  observed: boolean;
}

export interface MapFreshness {
  /** Runs read per function at most. */
  perFunction: number;
  /** Runs drawn from, across every mapped function. */
  runs: number;
  oldest: string | null;
  newest: string | null;
  retentionDays: number;
  /** Mapped functions with no recorded run in the window. */
  quiet: string[];
}

export interface AppMap {
  entries: MapEntry[];
  functions: MapFunction[];
  /** In `TARGET_GROUPS` order; within a group, beside the functions that reach it. */
  targets: MapTarget[];
  edges: MapEdge[];
  /** `null` while the traces have not been read: the right side is manifest-only. */
  freshness: MapFreshness | null;
  /** Declared secrets no recorded call names. A function reads its secrets through
   *  `ctx.env`, which is not a traced call, so the map cannot say who reads these. */
  untracedSecrets: string[];
}

export interface AppMapInput {
  /** The build the environment serves, as the Functions panel lists it. */
  functions: AppFunctionSummary[];
  /** `undefined` while not read (or unreadable). */
  traces?: RecentTraces;
  /** The environment's secrets view, when read. */
  secrets?: AppSecretEntry[];
}

export const functionId = (name: string) => `fn:${name}`;

/** What starts `fn`, in a fixed order so a function's band reads the same everywhere. */
export function entriesOf(fn: AppFunctionSummary): MapEntry[] {
  const entry = (kind: EntryKind, label: string, detail: string | null): MapEntry => ({
    id: `entry:${fn.name}:${kind}`,
    fn: fn.name,
    kind,
    label,
    detail
  });
  const out: MapEntry[] = [];
  if (fn.route) out.push(entry("route", "HTTP", `fn/${fn.name}`));
  if (fn.webhook) out.push(entry("webhook", "Webhook", fn.webhook.signature_header ?? null));
  if (fn.schedule) {
    const detail = fn.timezone ? `${fn.schedule} · ${fn.timezone}` : fn.schedule;
    out.push(entry("schedule", "Schedule", detail));
  }
  if (fn.airway) out.push(entry("airway", "Airway step", null));
  if (fn.check) out.push(entry("check", "Check", null));
  if (out.length === 0) out.push(entry("manual", "Run now only", null));
  return out;
}

interface Placement {
  group: TargetGroup;
  /** Identity within the group. */
  key: string;
  label: string;
  /** What this call adds to the node's `details`. */
  detail: string | null;
}

/** Where a call lands on the right, by family. A target a trace did not record
 *  still gets a node — "it sent email" is worth drawing without a recipient. */
export function placementOf(touch: Pick<TraceTouch, "family" | "target" | "verb">): Placement {
  const { family, target, verb } = touch;
  const named = (group: TargetGroup, fallback: string): Placement => ({
    group,
    key: target ?? "",
    label: target ?? fallback,
    detail: verb ?? null
  });
  switch (family) {
    case "warehouse": {
      // `database.table` or the database alone: one node per database, which is
      // what the manifest's `destinations` name, with its tables as detail.
      const [database, ...table] = (target ?? "").split(".");
      return {
        group: "warehouse",
        key: database,
        label: database || "Warehouse (database not recorded)",
        detail: table.length > 0 ? table.join(".") : null
      };
    }
    case "query":
    case "query_stream":
      return named("tables", "SQL, no table named");
    case "oltp":
    case "tx":
      return named("oltp", "OLTP");
    case "airhouse":
      return named("airhouse", "Airhouse");
    case "storage":
      return named("storage", "App storage");
    case "fetch":
      return named("fetch", "Host not recorded");
    case "airway":
      return named("airway", "Airway pipeline");
    case "semantic":
      return { group: "semantic", key: "", label: "Semantic layer", detail: null };
    case "email":
      return { group: "messaging", key: "email", label: "Email", detail: null };
    case "notify":
      return { group: "messaging", key: "notify", label: "Notifications", detail: null };
    case "secrets":
      return named("secrets", "Secrets (key not recorded)");
    case "org":
      return { group: "platform", key: "org", label: "Org directory", detail: null };
    case "platform":
      return { group: "platform", key: "platform", label: "Platform facts", detail: null };
    default:
      return {
        group: "other",
        key: `${family}:${target ?? ""}`,
        label: target ? `${family} · ${target}` : family,
        detail: verb ?? null
      };
  }
}

const targetId = (group: TargetGroup, key: string) => `target:${group}:${key}`;

const pushOnce = (list: string[], value: string | null | undefined) => {
  if (value && !list.includes(value)) list.push(value);
};

/** Mutable accumulators while folding; frozen into the public shapes at the end. */
interface Builder {
  targets: Map<string, MapTarget>;
  edges: Map<string, MapEdge>;
}

function targetFor(b: Builder, place: Placement): MapTarget {
  const id = targetId(place.group, place.key);
  let target = b.targets.get(id);
  if (!target) {
    target = {
      id,
      group: place.group,
      label: place.label,
      details: [],
      calls: 0,
      failedCalls: 0,
      declared: false,
      observed: false
    };
    b.targets.set(id, target);
  }
  return target;
}

function touchEdge(b: Builder, fn: string, to: string): MapEdge {
  const from = functionId(fn);
  const id = `${from}->${to}`;
  let edge = b.edges.get(id);
  if (!edge) {
    edge = {
      id,
      from,
      to,
      kind: "touch",
      families: [],
      calls: 0,
      failedCalls: 0,
      declared: false,
      observed: false
    };
    b.edges.set(id, edge);
  }
  return edge;
}

function addDestinations(b: Builder, fn: AppFunctionSummary) {
  for (const database of fn.destinations) {
    const target = targetFor(b, {
      group: "warehouse",
      key: database,
      label: database,
      detail: null
    });
    target.declared = true;
    touchEdge(b, fn.name, target.id).declared = true;
  }
}

function addTouch(b: Builder, fn: string, touch: TraceTouch) {
  const place = placementOf(touch);
  const target = targetFor(b, place);
  const edge = touchEdge(b, fn, target.id);
  const failed = touch.status === "error" ? touch.calls : 0;
  pushOnce(target.details, place.detail);
  target.calls += touch.calls;
  target.failedCalls += failed;
  target.observed = true;
  pushOnce(edge.families, touch.family);
  edge.calls += touch.calls;
  edge.failedCalls += failed;
  edge.observed = true;
}

function freshnessOf(traces: RecentTraces, mapped: Set<string>): MapFreshness {
  const times: string[] = [];
  const quiet: string[] = [];
  for (const { function: name, traces: runs } of traces.functions) {
    if (!mapped.has(name)) continue;
    if (runs.length === 0) quiet.push(name);
    for (const run of runs) times.push(run.created_at);
  }
  // Mapped functions the server did not answer for at all are as quiet as an
  // empty entry: the build changed between the two reads.
  const answered = new Set(traces.functions.map((f) => f.function));
  for (const name of mapped) if (!answered.has(name)) quiet.push(name);
  times.sort((a, b) => Date.parse(a) - Date.parse(b));
  return {
    perFunction: traces.per_function,
    runs: times.length,
    oldest: times[0] ?? null,
    newest: times[times.length - 1] ?? null,
    retentionDays: traces.retention_days,
    quiet: quiet.sort()
  };
}

/** Each group's targets beside the functions that reach them: by the mean row of
 *  those functions (a barycentre), which keeps most edges from crossing. */
function orderTargets(targets: MapTarget[], edges: MapEdge[], functions: string[]): MapTarget[] {
  const row = new Map(functions.map((name, i) => [functionId(name), i]));
  const rows = new Map<string, number[]>();
  for (const edge of edges) {
    const r = row.get(edge.from);
    if (edge.kind !== "touch" || r === undefined) continue;
    rows.set(edge.to, [...(rows.get(edge.to) ?? []), r]);
  }
  const centre = (id: string) => {
    const of = rows.get(id) ?? [];
    return of.length === 0 ? 0 : of.reduce((a, b) => a + b, 0) / of.length;
  };
  const groupRank = (g: TargetGroup) => TARGET_GROUPS.indexOf(g);
  return [...targets].sort(
    (a, b) =>
      groupRank(a.group) - groupRank(b.group) ||
      centre(a.id) - centre(b.id) ||
      a.label.localeCompare(b.label)
  );
}

/** A webhook's signing key is left out: the platform reads it to verify a
 *  delivery before the function runs, so it is the webhook entry's, not a read. */
function untracedSecretsOf(secrets: AppSecretEntry[] | undefined, targets: MapTarget[]): string[] {
  if (!secrets) return [];
  const named = new Set(targets.filter((t) => t.group === "secrets").map((t) => t.label));
  return secrets
    .filter((s) => s.declared && s.source !== "webhook" && !named.has(s.key))
    .map((s) => s.key)
    .sort();
}

/** Manifest + recorded traces → the map. Traces of a function the build no longer
 *  has are left out: the map is of the build the environment serves. */
export function buildAppMap({ functions, traces, secrets }: AppMapInput): AppMap {
  const b: Builder = { targets: new Map(), edges: new Map() };
  const mapped = new Set(functions.map((f) => f.name));
  const runsOf = new Map(traces?.functions.map((f) => [f.function, f.traces]) ?? []);

  const entries = functions.flatMap(entriesOf);
  const triggers: MapEdge[] = entries.map((entry) => ({
    id: `${entry.id}->${functionId(entry.fn)}`,
    from: entry.id,
    to: functionId(entry.fn),
    kind: "trigger",
    families: [],
    calls: 0,
    failedCalls: 0,
    declared: true,
    observed: false
  }));

  for (const fn of functions) {
    addDestinations(b, fn);
    for (const run of runsOf.get(fn.name) ?? []) {
      for (const touch of run.touches) addTouch(b, fn.name, touch);
    }
  }

  const mapFunctions: MapFunction[] = functions.map((fn) => {
    const runs = runsOf.get(fn.name);
    return {
      id: functionId(fn.name),
      name: fn.name,
      runs: traces ? (runs?.length ?? 0) : null,
      failedRuns: runs?.filter((r) => r.failed).length ?? 0
    };
  });

  const touches = [...b.edges.values()];
  const targets = orderTargets(
    [...b.targets.values()],
    touches,
    functions.map((f) => f.name)
  );
  return {
    entries,
    functions: mapFunctions,
    targets,
    edges: [...triggers, ...touches],
    freshness: traces ? freshnessOf(traces, mapped) : null,
    untracedSecrets: traces ? untracedSecretsOf(secrets, targets) : []
  };
}
