import type { Fleet } from "@/services/api/adminObservability";

/**
 * Which columns the fleet list can actually answer.
 *
 * The same rule `AdminCustomApps/fleetColumns.ts` wrote, applied to a second
 * table: **a column nothing can answer is absent and named once, never `—` on
 * every row.** Five app columns times every org on the platform is a screen of
 * em-dashes telling the operator nothing, in the half of the table they came
 * to read.
 *
 * And the same exception, for the same reason: the app columns gate on the
 * **deployment capability** (`apps_measured`), not on the values. With a store
 * configured, an org sitting at 0 requests is real information — nobody opened
 * its apps. With no store, the identical zeros are an artefact. Reading the
 * values cannot tell those apart; the flag can.
 *
 * Cost is never derived away. It comes from Postgres, which every deployment
 * has, so "no spend" is always a measurement rather than an absence.
 */
export type FleetColumnId =
  | "org"
  | "spend"
  | "runRate"
  | "costPerRun"
  | "unattended"
  | "cache"
  | "requests"
  | "p95"
  | "errorRate";

export interface FleetColumn {
  id: FleetColumnId;
  label: string;
  /** Right-aligned, tabular-nums — figures read down a column. */
  numeric?: boolean;
  /** One line of help, shown as the header's title attribute. */
  hint?: string;
}

export interface HiddenColumns {
  labels: string[];
  /** Why they are absent, as a clause completing "… because <why>". */
  why: string;
}

/** Identity and spend: the two things this list exists to show. */
const COST_COLUMNS: FleetColumn[] = [
  { id: "org", label: "Organization" },
  { id: "spend", label: "Spend", numeric: true, hint: "Priced LLM cost over the window" },
  {
    id: "runRate",
    label: "Run-rate",
    numeric: true,
    hint: "This window's spend scaled to 30 days"
  },
  { id: "costPerRun", label: "$/run", numeric: true, hint: "Priced cost ÷ runs" },
  {
    id: "unattended",
    label: "Unattended",
    numeric: true,
    hint: "Share of runs a schedule started — spend that grows without anyone deciding to"
  },
  {
    id: "cache",
    label: "Cache",
    numeric: true,
    hint: "Share of input tokens served from cache — the main cost lever"
  }
];

const APP_COLUMNS: FleetColumn[] = [
  { id: "requests", label: "Requests", numeric: true, hint: "Custom-app requests served" },
  {
    id: "p95",
    label: "p95",
    numeric: true,
    hint: "95th-percentile duration of successful requests"
  },
  {
    id: "errorRate",
    label: "Errors",
    numeric: true,
    hint: "Share of requests that did not succeed"
  }
];

export function fleetColumns(fleet: Pick<Fleet, "apps_measured">): {
  shown: FleetColumn[];
  hidden: HiddenColumns | null;
} {
  if (fleet.apps_measured) {
    return { shown: [...COST_COLUMNS, ...APP_COLUMNS], hidden: null };
  }
  return {
    shown: COST_COLUMNS,
    hidden: {
      labels: APP_COLUMNS.map((c) => c.label),
      why: "this deployment has no observability store (OXY_OBSERVABILITY_BACKEND is unset)"
    }
  };
}
