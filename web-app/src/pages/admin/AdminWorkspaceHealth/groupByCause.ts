import type {
  WorkspaceHealthDimensionKey,
  WorkspaceHealthEntry,
  WorkspaceHealthStatus
} from "@/services/api/workspaceHealth";

/**
 * Turn a worst-first list of workspaces into a worst-first list of **causes**.
 *
 * The rollup is fleet-wide, and a fleet breaks in fleet-shaped ways: one refused
 * Postgres connection, one unconfigured connector, one bad `.app.yml` — each landing on
 * every workspace that touches it. Listed per workspace, that read as nine separate
 * incidents. On the seeded fleet it is nine rows, nine identical 206-character failure
 * strings, one per line, each overflowing the viewport — and an operator who has to
 * compare nine truncated strings by eye to work out they are the same string.
 *
 * Grouping inverts it: one cause, the affected workspaces underneath, and the operator
 * counts incidents instead of rows.
 *
 * Identity is the reason text with its volatile tail normalized off — the backend
 * appends "(+22 more failing probe(s))", and a workspace failing 22 probes and one
 * failing 23 are the same incident.
 */
export type HealthCause = {
  /** Stable key for React and for `data-testid`. */
  id: string;
  /** The failure, in full, with the probe-count tail removed. */
  reason: string;
  /** Worst status among the affected workspaces. */
  status: WorkspaceHealthStatus;
  /** Which dimensions reported this reason, for "what subsystem is this?". */
  dimensions: WorkspaceHealthDimensionKey[];
  /** Affected workspaces, in the order the backend ranked them. */
  workspaces: WorkspaceHealthEntry[];
};

const RANK: Record<WorkspaceHealthStatus, number> = { unhealthy: 0, degraded: 1, healthy: 2 };

/**
 * Strip the trailing probe count so two workspaces failing the same way group together.
 * Everything else is left verbatim — an id, a table name or a port in the message is a
 * real distinction between two incidents, not noise.
 */
export function normalizeReason(reason: string): string {
  return reason.replace(/\s*\(\+\d+ more failing probe\(s\)\)\s*$/, "").trim();
}

/** The worse of two statuses. */
export function worseStatus(
  a: WorkspaceHealthStatus,
  b: WorkspaceHealthStatus
): WorkspaceHealthStatus {
  return RANK[a] <= RANK[b] ? a : b;
}

export function groupByCause(workspaces: WorkspaceHealthEntry[]): HealthCause[] {
  const byReason = new Map<string, HealthCause>();

  for (const ws of workspaces) {
    // A workspace with no reason is not an incident — it is a healthy workspace, and the
    // caller reports those as a count. `unhealthy` with an empty `reasons` would be a
    // backend bug; surface it rather than silently dropping the row.
    const reasons =
      ws.reasons.length > 0 ? ws.reasons : ws.status === "healthy" ? [] : ["(no reason reported)"];

    for (const raw of new Set(reasons.map(normalizeReason))) {
      const existing = byReason.get(raw);
      // Which dimension(s) carried this reason — a substring match, because the rollup
      // reason is the dimension's reason with the probe tail appended.
      const dims = ws.dimensions
        .filter((d) => d.reason !== null && normalizeReason(d.reason).startsWith(raw.slice(0, 60)))
        .map((d) => d.dimension);

      if (existing) {
        existing.workspaces.push(ws);
        existing.status = worseStatus(existing.status, ws.status);
        for (const d of dims) if (!existing.dimensions.includes(d)) existing.dimensions.push(d);
      } else {
        byReason.set(raw, {
          id: raw,
          reason: raw,
          status: ws.status,
          dimensions: [...new Set(dims)],
          workspaces: [ws]
        });
      }
    }
  }

  // Worst first, then widest blast radius — the order an operator works down.
  return [...byReason.values()].sort(
    (a, b) => RANK[a.status] - RANK[b.status] || b.workspaces.length - a.workspaces.length
  );
}

/** Human label for a dimension key, for the chip beside a cause. */
export const DIMENSION_LABEL: Record<WorkspaceHealthDimensionKey, string> = {
  job_liveness: "Jobs",
  pipeline: "Pipeline",
  queue: "Queue",
  reconciliation: "Reconciliation",
  smoke_test: "Smoke test",
  custom_app_availability: "Custom apps"
};
