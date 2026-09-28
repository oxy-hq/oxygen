import {
  Activity,
  Workflow as Automation,
  Ban,
  Bot,
  CalendarClock,
  CheckCircle2,
  CircleDashed,
  CircleDot,
  Code2,
  Database,
  History,
  Loader2,
  PauseCircle,
  Play,
  RotateCcw,
  Upload,
  XCircle
} from "lucide-react";
import type React from "react";

/**
 * Shared vocabulary for the coordinator dashboard. Job *type* and run
 * *status* are filters, never tabs — every surface composes from these
 * configs so a status dot looks identical on the Overview timeline, the
 * Runs table, and a run-detail header.
 */

// ── Run status ──────────────────────────────────────────────────────────────

export type RunStatus =
  | "running"
  | "suspended"
  | "done"
  | "failed"
  | "cancelled"
  | "queued"
  | "skipped"
  | "missing";

interface StatusMeta {
  label: string;
  /** Foreground text/icon color token. */
  fg: string;
  /** Soft background token for solid badges and timeline bars. */
  bg: string;
  icon: React.ElementType;
  spin?: boolean;
}

export const RUN_STATUS: Record<RunStatus, StatusMeta> = {
  running: { label: "Running", fg: "text-primary", bg: "bg-primary", icon: Loader2, spin: true },
  suspended: { label: "Suspended", fg: "text-warning", bg: "bg-warning", icon: PauseCircle },
  done: { label: "Succeeded", fg: "text-success", bg: "bg-success", icon: CheckCircle2 },
  failed: { label: "Failed", fg: "text-destructive", bg: "bg-destructive", icon: XCircle },
  cancelled: {
    label: "Cancelled",
    fg: "text-muted-foreground",
    bg: "bg-muted-foreground",
    icon: Ban
  },
  queued: {
    label: "Queued",
    fg: "text-muted-foreground",
    bg: "bg-muted-foreground",
    icon: CircleDot
  },
  skipped: {
    label: "Skipped",
    fg: "text-muted-foreground",
    bg: "bg-muted-foreground",
    icon: CircleDashed
  },
  missing: { label: "Missing", fg: "text-warning", bg: "bg-warning", icon: CircleDashed }
};

/** Normalize any backend status string into a known RunStatus. */
export const normalizeStatus = (raw: string | null | undefined): RunStatus => {
  switch (raw) {
    case "running":
    case "delegating":
      return "running";
    case "suspended":
    case "awaiting_input":
      return "suspended";
    case "done":
      return "done";
    case "failed":
    case "timed_out":
      return "failed";
    case "cancelled":
      return "cancelled";
    case "queued":
      return "queued";
    case "skipped":
      return "skipped";
    case "missing":
      return "missing";
    default:
      return "running";
  }
};

// ── Job type ────────────────────────────────────────────────────────────────

export type JobType = "agent" | "dag" | "elt" | "monitor" | "function";

interface JobTypeMeta {
  label: string;
  /** Short label for dense surfaces. */
  short: string;
  fg: string;
  bg: string;
  /** Soft tinted background for badges. */
  tint: string;
  icon: React.ElementType;
  /** The debugging unit, surfaced in empty states and detail headers. */
  unit: string;
}

export const JOB_TYPE: Record<JobType, JobTypeMeta> = {
  agent: {
    label: "LLM Agent",
    short: "Agent",
    fg: "text-primary",
    bg: "bg-primary",
    tint: "bg-primary/10 text-primary",
    icon: Bot,
    unit: "trace"
  },
  dag: {
    label: "DAG Automation",
    short: "DAG",
    fg: "text-vis-purple",
    bg: "bg-vis-purple",
    tint: "bg-vis-purple/10 text-vis-purple",
    icon: Automation,
    unit: "task graph"
  },
  elt: {
    label: "ELT Pipeline",
    short: "ELT",
    fg: "text-vis-cyan",
    bg: "bg-vis-cyan",
    tint: "bg-vis-cyan/10 text-vis-cyan",
    icon: Database,
    unit: "rows + freshness"
  },
  monitor: {
    label: "Monitor scan",
    short: "Monitor",
    fg: "text-vis-amber",
    bg: "bg-vis-amber",
    tint: "bg-vis-amber/10 text-vis-amber",
    icon: Activity,
    unit: "anomaly scan"
  },
  function: {
    label: "Function Job",
    short: "Function",
    fg: "text-vis-violet",
    bg: "bg-vis-violet",
    tint: "bg-vis-violet/10 text-vis-violet",
    icon: Code2,
    unit: "run log"
  }
};

export const JOB_TYPES: JobType[] = ["agent", "dag", "elt", "monitor", "function"];

/** Map a schedule `target_kind` to a job type. */
export const targetKindToJobType = (kind: string): JobType => {
  if (kind === "airway") return "elt";
  if (kind === "agent") return "agent";
  if (kind === "monitor_scan") return "monitor";
  if (kind === "function") return "function";
  return "dag";
};

/** Map a run `source_type` to a job type. */
export const sourceTypeToJobType = (source: string | null | undefined): JobType => {
  switch (source) {
    case "workflow":
      return "dag";
    case "airway":
      return "elt";
    case "monitor_scan":
      return "monitor";
    case "app_function":
      return "function";
    default:
      return "agent";
  }
};

// ── System internals ────────────────────────────────────────────────────────
//
// Background daemons that produce runs but aren't user-scheduled jobs:
// e.g. the preagg refresh worker, which fires a fresh rollup cycle every
// heartbeat. They aren't agent / DAG / ELT — the job-type badge
// misclassifies them. The UI replaces the JobTypeBadge with a SystemBadge
// for these so operators don't mistake heartbeat noise for their work.

/** Run source_types that are internal daemons, not user-scheduled jobs. */
const SYSTEM_SOURCE_TYPES: readonly string[] = ["preagg_cycle"];

export const isSystemSource = (source: string | null | undefined): boolean =>
  !!source && SYSTEM_SOURCE_TYPES.includes(source);

/** Schedule `target_kind`s the platform manages rather than a person: the row
 *  exists so the CADENCE is configurable, but the work behind it refreshes to
 *  the present — a monitor scan reads the warehouse as it is now; a health
 *  evaluation and a pre-aggregation cycle bring a cache up to date.
 *
 *  Two things follow, and both are why this list is shared rather than
 *  re-spelled at each site: the job-type badge would misclassify them (they are
 *  not agent / DAG / ELT), and there is no per-occurrence run to replay, so the
 *  server rejects a backfill for them and the UI must not offer one. */
export const SYSTEM_MANAGED_TARGET_KINDS: readonly string[] = [
  "monitor_scan",
  "health_eval",
  "preagg_cycle"
];

/** Whether a schedule is a system-managed job rather than user-created. */
export const isSystemSchedule = (schedule: { target_kind?: string; name?: string }): boolean =>
  !!schedule.target_kind && SYSTEM_MANAGED_TARGET_KINDS.includes(schedule.target_kind);

// ── Trigger source ──────────────────────────────────────────────────────────

export type Trigger = "scheduled" | "manual" | "backfill" | "retry" | "promote";

interface TriggerMeta {
  label: string;
  fg: string;
  tint: string;
  icon: React.ElementType;
}

export const TRIGGER: Record<Trigger, TriggerMeta> = {
  scheduled: {
    label: "Scheduled",
    fg: "text-muted-foreground",
    tint: "bg-muted text-foreground",
    icon: CalendarClock
  },
  manual: {
    label: "Manual",
    fg: "text-primary",
    tint: "bg-primary/10 text-primary",
    icon: Play
  },
  backfill: {
    label: "Backfill",
    fg: "text-warning",
    tint: "bg-warning/10 text-warning",
    icon: History
  },
  retry: {
    label: "Retry",
    fg: "text-primary",
    tint: "bg-primary/10 text-primary",
    icon: RotateCcw
  },
  // A promoted compile seeded this itself, because what it promoted changed
  // the definition the job builds from. Neither "Scheduled" (the cadence did
  // not fire) nor "Manual" (nobody clicked) — and reading as neither, which
  // an unrecognized trigger does, is what left a pending build invisible.
  promote: {
    label: "Promote",
    fg: "text-foreground",
    tint: "bg-muted text-foreground",
    icon: Upload
  }
};

/** Normalize a backend trigger string into the typed union (or null). */
export const normalizeTrigger = (raw: string | null | undefined): Trigger | null => {
  switch (raw) {
    case "scheduled":
    case "manual":
    case "backfill":
    case "retry":
    case "promote":
      return raw;
    default:
      return null;
  }
};

// ── Time range ──────────────────────────────────────────────────────────────

export type TimeRange = "1h" | "24h" | "7d";

export const TIME_RANGES: { value: TimeRange; label: string; ms: number }[] = [
  { value: "1h", label: "1h", ms: 60 * 60 * 1000 },
  { value: "24h", label: "24h", ms: 24 * 60 * 60 * 1000 },
  { value: "7d", label: "7d", ms: 7 * 24 * 60 * 60 * 1000 }
];

export const rangeMs = (r: TimeRange): number =>
  TIME_RANGES.find((t) => t.value === r)?.ms ?? TIME_RANGES[1].ms;
