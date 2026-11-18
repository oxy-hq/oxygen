import type { AppEventListenerHealth, AppEventType } from "@/services/api/appEvents";

/**
 * What the Events page shows for one app in one environment, derived from the org's
 * `events/types` reply. Pure, so the rules below are tested rather than eyeballed.
 *
 * - An app is involved with a type when it **publishes** it, or one of its functions
 *   **listens** to it. Types it has nothing to do with are left out: the org's reply
 *   lists every app's.
 * - For a type the app publishes, every listener counts, whichever app it is in: a
 *   stuck listener elsewhere is this publisher's backlog too. For a type it only
 *   listens to, only its own listeners count; another app's are that app's business.
 * - Only listeners in the environment the page shows count. The same function in
 *   staging and in production is two listeners with two queues.
 */

export type EventRole = "publishes" | "listens" | "both";

export type QueueCounts = {
  pending: number;
  retrying: number;
  dead: number;
  held: number;
  inFlight: number;
  /** Seconds the oldest pending delivery has waited; `null` when nothing is pending. */
  oldestPendingSeconds: number | null;
};

export type EventTypeRow = {
  type: string;
  publisher: string;
  event: string;
  role: EventRole;
  schemaRevision: number | null;
  lastPublishedAt: string | null;
  listeners: AppEventListenerHealth[];
  counts: QueueCounts;
};

export type EventsView = {
  rows: EventTypeRow[];
  totals: QueueCounts;
};

export const EMPTY_COUNTS: QueueCounts = {
  pending: 0,
  retrying: 0,
  dead: 0,
  held: 0,
  inFlight: 0,
  oldestPendingSeconds: null
};

const oldest = (a: number | null, b: number | null): number | null =>
  a === null ? b : b === null ? a : Math.max(a, b);

export const sumCounts = (listeners: readonly AppEventListenerHealth[]): QueueCounts =>
  listeners.reduce<QueueCounts>(
    (acc, l) => ({
      pending: acc.pending + l.pending,
      retrying: acc.retrying + l.retrying,
      dead: acc.dead + l.dead,
      held: acc.held + l.held,
      inFlight: acc.inFlight + l.inFlight,
      oldestPendingSeconds: oldest(acc.oldestPendingSeconds, l.oldestPendingAgeSeconds)
    }),
    EMPTY_COUNTS
  );

const addCounts = (a: QueueCounts, b: QueueCounts): QueueCounts => ({
  pending: a.pending + b.pending,
  retrying: a.retrying + b.retrying,
  dead: a.dead + b.dead,
  held: a.held + b.held,
  inFlight: a.inFlight + b.inFlight,
  oldestPendingSeconds: oldest(a.oldestPendingSeconds, b.oldestPendingSeconds)
});

/** Worst first: dead, then held, then the longest wait, then by name. */
const byAttention = (a: EventTypeRow, b: EventTypeRow): number =>
  b.counts.dead - a.counts.dead ||
  b.counts.held - a.counts.held ||
  (b.counts.oldestPendingSeconds ?? -1) - (a.counts.oldestPendingSeconds ?? -1) ||
  a.type.localeCompare(b.type);

export const eventsView = (
  types: readonly AppEventType[],
  appSlug: string,
  environment: string
): EventsView => {
  const rows: EventTypeRow[] = [];
  for (const t of types) {
    const publishes = t.publisher === appSlug;
    const inEnvironment = t.listeners.filter((l) => l.environment === environment);
    const own = inEnvironment.filter((l) => l.app === appSlug);
    // A type this app only listens to, but not in this environment, is not on this page.
    const listens = t.listeners.some((l) => l.app === appSlug);
    if (!publishes && !listens) continue;
    if (!publishes && own.length === 0) continue;
    const listeners = publishes ? inEnvironment : own;
    rows.push({
      type: t.type,
      publisher: t.publisher,
      event: t.event,
      role: publishes && own.length > 0 ? "both" : publishes ? "publishes" : "listens",
      schemaRevision: t.schemaRevision,
      lastPublishedAt: t.lastPublishedAt,
      listeners: [...listeners].sort(
        (a, b) => b.dead - a.dead || a.listener.localeCompare(b.listener)
      ),
      counts: sumCounts(listeners)
    });
  }
  rows.sort(byAttention);
  return { rows, totals: rows.reduce((acc, r) => addCounts(acc, r.counts), EMPTY_COUNTS) };
};

export type EventsFilter = { query: string; pendingOnly: boolean; deadOnly: boolean };

/** The rows a search and the two toggles leave. The toggles narrow; they never widen. */
export const filterEventRows = (rows: readonly EventTypeRow[], f: EventsFilter): EventTypeRow[] => {
  const q = f.query.trim().toLowerCase();
  return rows.filter(
    (r) =>
      (!q ||
        r.type.toLowerCase().includes(q) ||
        r.listeners.some((l) => l.listener.toLowerCase().includes(q))) &&
      (!f.pendingOnly || r.counts.pending > 0) &&
      (!f.deadOnly || r.counts.dead > 0)
  );
};

/** "45s", "12m", "3h", "2d": how long the oldest delivery has waited. */
export const formatWait = (seconds: number | null): string | null => {
  if (seconds === null) return null;
  if (seconds < 60) return `${Math.max(0, Math.round(seconds))}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  if (seconds < 86_400) return `${Math.round(seconds / 3600)}h`;
  return `${Math.round(seconds / 86_400)}d`;
};
