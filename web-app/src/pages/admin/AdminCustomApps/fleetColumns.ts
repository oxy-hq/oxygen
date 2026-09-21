import type { AppStorageUsageRow, CustomApp } from "@/types/apps";

/**
 * Which columns the fleet can actually answer.
 *
 * The old fleet table hard-coded five columns and let two of them be `—` on every
 * row, forever: `Requests (6h)` and `Last active` cannot be anything else while
 * observability capture is off, which on this deployment it is. Three rows times two
 * columns is six em-dashes telling the operator nothing, in the two widest columns
 * on screen.
 *
 * So the column set is *derived* rather than declared. One rule:
 *
 *   **A column is shown iff at least one app can answer it.**
 *
 * with one deliberate exception. `requests` gates on the deployment capability, not on
 * the values — because with capture ON, every app sitting at 0 requests is real
 * information ("nobody used any of these"), while with capture OFF the same zeros are
 * an artefact. Reading the values alone cannot tell those apart; the flag can.
 *
 * What is dropped is **named once, with the reason**, rather than silently vanishing.
 * A column that disappears with no explanation is its own small dishonesty — the
 * operator cannot tell "this deployment cannot measure that" from "someone removed the
 * column". One line at the foot of the table beats `n` rows of dashes and beats silence.
 */
export type FleetColumnId = "status" | "app" | "published" | "lastActive" | "requests" | "storage";

export interface FleetColumn {
  id: FleetColumnId;
  label: string;
  /** Right-aligned, tabular-nums — counts and sizes read down a column. */
  numeric?: boolean;
}

export interface HiddenColumn {
  label: string;
  /** Why it is absent, as a clause that completes "… because <why>". */
  why: string;
}

export interface FleetColumnInput {
  apps: readonly CustomApp[];
  /** `false` when OXY_OBSERVABILITY_BACKEND is unset — see the exception above. */
  observabilityConfigured: boolean;
  /** Absent while the fleet storage rollup is still loading or failed. */
  storage: ReadonlyMap<string, AppStorageUsageRow> | undefined;
}

/**
 * Identity and verdict are not optional. A fleet list that cannot say *which app* or
 * *how it is* has stopped being a fleet list, so these two are never derived away even
 * on an empty fleet.
 */
const ALWAYS: FleetColumn[] = [
  { id: "status", label: "Status" },
  { id: "app", label: "App" }
];

export function fleetColumns({ apps, observabilityConfigured, storage }: FleetColumnInput): {
  shown: FleetColumn[];
  hidden: HiddenColumn[];
} {
  const shown: FleetColumn[] = [...ALWAYS];
  const hidden: HiddenColumn[] = [];

  const keep = (when: boolean, column: FleetColumn, why: string): void => {
    if (when) shown.push(column);
    else hidden.push({ label: column.label, why });
  };

  keep(
    apps.some((a) => Boolean(a.published_at)),
    { id: "published", label: "Published" },
    "nothing here has been published yet"
  );

  keep(
    apps.some((a) => Boolean(a.last_active_at)),
    { id: "lastActive", label: "Last active" },
    "no app has recorded a visit"
  );

  // The capability, not the values: all-zero requests means something different
  // depending on whether anything was watching.
  keep(
    observabilityConfigured,
    { id: "requests", label: "Requests", numeric: true },
    "nothing on this deployment is measured"
  );

  keep(
    Boolean(storage && apps.some((a) => storage.has(a.id))),
    { id: "storage", label: "Storage", numeric: true },
    "storage has not been measured yet"
  );

  return { shown, hidden };
}
