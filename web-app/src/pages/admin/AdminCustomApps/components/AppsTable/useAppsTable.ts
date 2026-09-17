import { useCallback, useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import type { AppHealth, CustomApp } from "@/types/apps";

/** Card grid vs. dense list — the two ways to look at the registry. */
export type ViewMode = "gallery" | "list";
export type GroupBy = "none" | "org" | "status";
export type SortKey = "name" | "org" | "status" | "active" | "updated";
export type SortDir = "asc" | "desc";

/**
 * What the status column says about one app.
 *
 * One column, not two. Publish state (live/draft) and fleet health used to be a
 * pill here and a separate tab, and they answer the same question — is this app
 * OK? — so a published app shows its health verdict and an unpublished one shows
 * `draft`. A draft serves nobody, so it has no health to report.
 */
export type AppStatus = AppHealth | "draft";

/**
 * `attention` is the working filter: the three verdicts that need someone.
 * `live` is kept so an old `?status=live` bookmark still means "not a draft".
 */
export type StatusFilter = "all" | "attention" | "live" | AppStatus;

/**
 * Health rows by app id. Typed to the one field the model reads, so a test can
 * pass `{ health }` while the page passes whole `AppHealthRow`s — the rows need
 * the reason and request count, the model does not.
 */
export type HealthIndex = ReadonlyMap<string, { health: AppHealth }>;

/** The verdicts an operator should look at — the same three the backend's
 *  `AppHealth::needs_attention` names, so the two cannot disagree. */
export const ATTENTION: ReadonlySet<AppStatus> = new Set<AppStatus>([
  "down",
  "degraded",
  "not_measured"
]);

/**
 * Worst first. An app whose health is not known yet ranks after every verdict
 * and before drafts: it may be fine, and a draft serves nobody.
 */
const STATUS_RANK: Record<AppStatus, number> = {
  down: 0,
  degraded: 1,
  not_measured: 2,
  quiet: 3,
  operational: 4,
  draft: 6
};
const UNKNOWN_RANK = 5;

/**
 * An app's status, or `null` when its health is not known.
 *
 * `null` covers two cases and deliberately does not guess between them: health
 * is still loading, or the app is past the fleet endpoint's page cap. Reporting
 * either as `not_measured` would claim capture is off for an app nobody asked
 * about — the same class of lie the fleet view exists to stop telling.
 */
export function statusOf(app: CustomApp, health: HealthIndex | undefined): AppStatus | null {
  if (!app.published_at) return "draft";
  return health?.get(app.id)?.health ?? null;
}

export interface AppsTableState {
  view: ViewMode;
  q: string;
  status: StatusFilter;
  /** An org slug, or `all`. */
  org: string;
  group: GroupBy;
  sortKey: SortKey;
  sortDir: SortDir;
}

export interface AppGroup {
  /** Stable key (collapse state, React key). Empty group uses `__all__`. */
  key: string;
  /** Header label; empty when `group === "none"` (no header rendered). */
  label: string;
  items: CustomApp[];
}

/** Per-status counts for the filter chips. */
export type StatusCounts = Record<AppStatus, number> & { attention: number; unknown: number };

export interface AppsTableModel {
  groups: AppGroup[];
  /** Visual top-to-bottom row order — drives select-all + shift-range. */
  flatIds: string[];
  filteredCount: number;
  totalCount: number;
  /**
   * Counts over everything the OTHER filters let through, ignoring the status
   * filter itself — so choosing "Down" does not zero every other chip, and a
   * chip's number always says what clicking it would show.
   */
  statusCounts: StatusCounts;
  /** Every org in the registry, for the org filter. Unfiltered on purpose: an
   *  option that vanished when another filter was set would read as "no apps". */
  orgs: string[];
}

/**
 * The landing an operator opens to triage: one ungrouped list, worst first.
 *
 * Grouping by org used to be the default, and it cut the worst-first ordering
 * into one short run per tenant — the down app in the fourth org sat below three
 * orgs of healthy ones. Org is a filter and a column now; grouping is still
 * available, just not the thing you land on.
 */
export const DEFAULT_TABLE_STATE: AppsTableState = {
  view: "list",
  q: "",
  status: "all",
  org: "all",
  group: "none",
  sortKey: "status",
  sortDir: "asc"
};

/** Sort keys that read most-useful newest-first, so a fresh click on them
 *  defaults to descending; the rest default to ascending. `status` ascends
 *  because its rank is worst-first. */
const DESC_FIRST: ReadonlySet<SortKey> = new Set(["active", "updated"]);

export const defaultDirFor = (key: SortKey): SortDir => (DESC_FIRST.has(key) ? "desc" : "asc");

/** Tri-state select for a group of rows: none → false, all → true, else
 *  "indeterminate". Shared by both views so their group headers can't drift. */
export const groupCheckState = (
  ids: string[],
  isSelected: (id: string) => boolean
): boolean | "indeterminate" => {
  const n = ids.filter(isSelected).length;
  return n === 0 ? false : n === ids.length ? true : "indeterminate";
};

/**
 * The one pure transform behind the whole table: filter → sort → group. Kept
 * side-effect-free (no hooks, no `Date.now` beyond the recency helpers) so it
 * unit-tests directly. `flatIds` is the concatenation of every group's items
 * in display order, so shift-click ranges and select-all stay honest whatever
 * the grouping.
 *
 * `health` is optional: without it every published app's status is unknown,
 * which is the truth while the fleet query is in flight.
 */
export function buildAppsTableModel(
  apps: CustomApp[],
  state: AppsTableState,
  health?: HealthIndex
): AppsTableModel {
  const status = (a: CustomApp) => statusOf(a, health);
  const needle = state.q.trim().toLowerCase();

  // Everything except the status filter, so the chips can count over it.
  const scoped = apps.filter((a) => {
    if (state.org !== "all" && a.org_slug !== state.org) return false;
    if (!needle) return true;
    return (
      a.name.toLowerCase().includes(needle) ||
      a.slug.toLowerCase().includes(needle) ||
      a.org_slug.toLowerCase().includes(needle) ||
      a.project_id.toLowerCase().includes(needle)
    );
  });
  const filtered = scoped.filter((a) => matchesStatus(status(a), state.status));

  const sign = state.sortDir === "asc" ? 1 : -1;
  const sorted = [...filtered].sort(
    (a, b) => sign * compareBy(state.sortKey, a, b, status) || a.name.localeCompare(b.name)
  );

  const groups =
    state.group === "none"
      ? [{ key: "__all__", label: "", items: sorted }]
      : partition(sorted, state.group, status);

  return {
    groups,
    flatIds: groups.flatMap((g) => g.items.map((a) => a.id)),
    filteredCount: filtered.length,
    totalCount: apps.length,
    statusCounts: countStatuses(scoped, status),
    orgs: [...new Set(apps.map((a) => a.org_slug))].sort()
  };
}

function matchesStatus(s: AppStatus | null, filter: StatusFilter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "attention":
      return s !== null && ATTENTION.has(s);
    case "live":
      return s !== "draft";
    default:
      return s === filter;
  }
}

function countStatuses(
  apps: CustomApp[],
  status: (a: CustomApp) => AppStatus | null
): StatusCounts {
  const counts: StatusCounts = {
    down: 0,
    degraded: 0,
    not_measured: 0,
    quiet: 0,
    operational: 0,
    draft: 0,
    attention: 0,
    unknown: 0
  };
  for (const a of apps) {
    const s = status(a);
    if (s === null) {
      counts.unknown += 1;
      continue;
    }
    counts[s] += 1;
    if (ATTENTION.has(s)) counts.attention += 1;
  }
  return counts;
}

const rankOf = (s: AppStatus | null): number => (s === null ? UNKNOWN_RANK : STATUS_RANK[s]);

function compareBy(
  key: SortKey,
  a: CustomApp,
  b: CustomApp,
  status: (a: CustomApp) => AppStatus | null
): number {
  switch (key) {
    case "name":
      return a.name.localeCompare(b.name);
    case "org":
      return a.org_slug.localeCompare(b.org_slug);
    case "status":
      return rankOf(status(a)) - rankOf(status(b));
    case "active":
      return activeScore(a) - activeScore(b);
    case "updated":
      return recencyScore(a) - recencyScore(b);
  }
}

function partition(
  sorted: CustomApp[],
  group: Exclude<GroupBy, "none">,
  status: (a: CustomApp) => AppStatus | null
): AppGroup[] {
  // Map preserves insertion order, so groups land ordered by their
  // top (already-sorted) row — the group holding the #1 row comes first.
  const map = new Map<string, CustomApp[]>();
  for (const a of sorted) {
    const key = groupKey(a, group, status);
    const list = map.get(key) ?? [];
    list.push(a);
    map.set(key, list);
  }
  return [...map.entries()].map(([key, items]) => ({
    key,
    label: groupLabel(key, group),
    items
  }));
}

function groupKey(
  a: CustomApp,
  group: Exclude<GroupBy, "none">,
  status: (a: CustomApp) => AppStatus | null
): string {
  return group === "org" ? a.org_slug : (status(a) ?? "unknown");
}

/** Sentence-case labels for every status, shared by the column, the chips and
 *  the group headers so the three cannot drift. */
export const STATUS_LABEL: Record<AppStatus, string> = {
  down: "Down",
  degraded: "Degraded",
  not_measured: "Not measured",
  quiet: "Quiet",
  operational: "Operational",
  draft: "Draft"
};

function groupLabel(key: string, group: Exclude<GroupBy, "none">): string {
  if (group === "org") return key;
  return STATUS_LABEL[key as AppStatus] ?? "Checking";
}

/** Millisecond epoch of `updated_at` (falls back to `created_at`); 0 on a
 *  bad/absent value so a broken row sinks instead of throwing. */
export function recencyScore(a: CustomApp): number {
  return epoch(a.updated_at ?? a.created_at);
}

/** "Is this being used?" score: last browser view, else last registry sync. */
export function activeScore(a: CustomApp): number {
  return epoch(a.last_active_at ?? a.last_synced_at ?? null);
}

function epoch(iso: string | null | undefined): number {
  if (!iso) return 0;
  const t = new Date(iso).getTime();
  return Number.isFinite(t) ? t : 0;
}

/** Compact "2m / 3h / 4d" stamp. `—` for an absent/unparseable value. */
export function formatRelativeTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "—";
  const sec = Math.round((Date.now() - then) / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h`;
  const day = Math.round(hr / 24);
  if (day < 30) return `${day}d`;
  return `${Math.round(day / 30)}mo`;
}

// ── URL-synced table state ────────────────────────────────────────────────

const oneOf = <T extends string>(v: string | null, allowed: readonly T[], fallback: T): T =>
  allowed.includes(v as T) ? (v as T) : fallback;

const STATUS_FILTERS: readonly StatusFilter[] = [
  "all",
  "attention",
  "live",
  "down",
  "degraded",
  "not_measured",
  "quiet",
  "operational",
  "draft"
];

/** Parse and write both read their defaults from here, so they cannot drift. */
const D = DEFAULT_TABLE_STATE;

function parseState(p: URLSearchParams): AppsTableState {
  return {
    view: oneOf(p.get("vw"), ["gallery", "list"], D.view),
    q: p.get("q") ?? D.q,
    status: oneOf(p.get("status"), STATUS_FILTERS, D.status),
    org: p.get("org") || D.org,
    group: oneOf(p.get("group"), ["none", "org", "status"], D.group),
    sortKey: oneOf(p.get("sort"), ["name", "org", "status", "active", "updated"], D.sortKey),
    sortDir: oneOf(p.get("dir"), ["asc", "desc"], D.sortDir)
  };
}

/** Write only non-default values so the URL stays clean and shareable. */
function writeState(p: URLSearchParams, s: AppsTableState) {
  const set = (k: string, v: string, def: string) => (v === def ? p.delete(k) : p.set(k, v));
  set("vw", s.view, D.view);
  set("q", s.q, D.q);
  set("status", s.status, D.status);
  set("org", s.org, D.org);
  set("group", s.group, D.group);
  set("sort", s.sortKey, D.sortKey);
  set("dir", s.sortDir, D.sortDir);
}

/**
 * Table filter/sort/group state, persisted in the URL query so a filtered view
 * is shareable and survives the back button. Only touches its own keys — the
 * `view` tab and selected-app path segments are left intact.
 */
export function useAppsTableState(): [AppsTableState, (patch: Partial<AppsTableState>) => void] {
  const [params, setParams] = useSearchParams();
  const state = useMemo(() => parseState(params), [params]);
  const setState = useCallback(
    (patch: Partial<AppsTableState>) => {
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          writeState(next, { ...parseState(prev), ...patch });
          return next;
        },
        { replace: true }
      );
    },
    [setParams]
  );
  return [state, setState];
}
