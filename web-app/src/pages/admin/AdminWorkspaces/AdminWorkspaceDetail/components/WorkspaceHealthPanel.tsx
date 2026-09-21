import { AlertTriangle, CheckCircle2, CircleAlert, HeartPulse, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { useTriggerWorkspaceHealthEval } from "@/hooks/api/workspaceHealth/useTriggerWorkspaceHealthEval";
import { useWorkspaceHealthEntry } from "@/hooks/api/workspaceHealth/useWorkspaceHealthEntry";
import { timeAgo } from "@/libs/utils/date";
import type {
  WorkspaceHealthDimensionKey,
  WorkspaceHealthReconciliationCheck,
  WorkspaceHealthSignals,
  WorkspaceHealthStatus
} from "@/services/api/workspaceHealth";
import { AdminAsync } from "../../../components/AdminAsync";
import { AdminDetailTabPanel } from "../../../components/AdminDetailTabs";
import { AdminEmptyState } from "../../../components/AdminEmptyState";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";
import { AdminStatusPill } from "../../../components/AdminStatusPill";
import { ADMIN_TONE } from "../../../components/adminTone";
import { workspaceHealthTone } from "../../../components/workspaceHealthTone";
import { SmokeTestSection } from "./SmokeTestSection";

const DIMENSION_LABELS: Record<WorkspaceHealthDimensionKey, string> = {
  job_liveness: "Jobs",
  pipeline: "Pipeline",
  queue: "Queue",
  reconciliation: "Reconciliation",
  smoke_test: "Smoke test",
  custom_app_availability: "Custom apps"
};

/** Numeric signal counts, in display order. Airway flags are rendered separately. */
const SIGNAL_ROWS: { key: keyof WorkspaceHealthSignals; label: string }[] = [
  { key: "total_runs", label: "Total runs (window)" },
  { key: "failed_runs", label: "Failed runs" },
  { key: "timed_out_runs", label: "Timed-out runs" },
  { key: "open_high_anomalies", label: "Open high-severity anomalies" },
  { key: "open_medium_anomalies", label: "Open medium-severity anomalies" },
  { key: "dead_letter_count", label: "Dead-letter tasks" }
];

// The icon takes its colour from the same tone the status pill beside it does, via
// `workspaceHealthTone` → `ADMIN_TONE`, so the two read as one unit and neither can
// drift. (This used to spell the shades out as raw emerald/amber, which the repo's
// style rule forbids and which carried its own hand-written `dark:` variants.)
const HealthIcon = ({ status }: { status: WorkspaceHealthStatus }) => {
  const tone = ADMIN_TONE[workspaceHealthTone(status)].text;
  switch (status) {
    case "healthy":
      return <CheckCircle2 className={`size-4 ${tone}`} />;
    case "degraded":
      return <AlertTriangle className={`size-4 ${tone}`} />;
    case "unhealthy":
      return <CircleAlert className={`size-4 ${tone}`} />;
  }
};

/**
 * Per-workspace Health tab. Derives its data from the cross-tenant rollup
 * (`useWorkspaceHealthEntry`) and surfaces three layers of detail beyond the
 * top-line status: the per-dimension breakdown, the "in this state since"
 * transition time, and the raw signal counts behind each dimension.
 */
export default function WorkspaceHealthPanel({ workspaceId }: { workspaceId: string }) {
  const health = useWorkspaceHealthEntry(workspaceId);
  const triggerEval = useTriggerWorkspaceHealthEval();

  return (
    <AdminDetailTabPanel>
      <AdminSectionLabel
        trailing={
          <Button
            variant='outline'
            size='sm'
            className='gap-1.5'
            onClick={() => triggerEval.mutate({ workspaceId })}
            disabled={triggerEval.isPending}
          >
            <RefreshCw className={triggerEval.isPending ? "size-3.5 animate-spin" : "size-3.5"} />
            Run health check
          </Button>
        }
      >
        Health
      </AdminSectionLabel>

      {/* The hook resolves to `null` — never `undefined` — for a workspace absent from
          the rollup, which is the ordinary case for a workspace with no `health_check:`
          block. So "not evaluated" is the EMPTY state here, and `undefined` is left to
          mean what it should: the rollup itself failed to load. */}
      <AdminAsync
        query={health}
        noun='workspace health'
        rows={3}
        isEmpty={(entry) => entry === null}
        empty={
          <AdminEmptyState
            icon={HeartPulse}
            title='No health data'
            description='This workspace does not appear in the current health rollup.'
          />
        }
      >
        {(health) =>
          health && (
            <div className='space-y-6'>
              <section className='space-y-4 rounded-lg border border-border/60 bg-card p-6'>
                <div className='flex flex-wrap items-center gap-3'>
                  <HealthIcon status={health.status} />
                  <AdminStatusPill
                    tone={workspaceHealthTone(health.status)}
                    label={health.status}
                  />
                  {health.changed_at ? (
                    <span className='text-muted-foreground text-xs'>
                      in this state since {new Date(health.changed_at).toLocaleString()}
                    </span>
                  ) : null}
                  {health.checked_at ? (
                    <span
                      className='text-muted-foreground/70 text-xs'
                      title={new Date(health.checked_at).toLocaleString()}
                    >
                      · last checked {timeAgo(health.checked_at)}
                    </span>
                  ) : (
                    <span className='text-muted-foreground/70 text-xs'>· awaiting first check</span>
                  )}
                </div>

                <div className='space-y-2'>
                  <AdminSectionLabel>Dimensions</AdminSectionLabel>
                  <ul className='space-y-1.5'>
                    {health.dimensions.map((d) => (
                      <li
                        key={d.dimension}
                        className='flex items-center justify-between gap-3 rounded-md border border-border/50 px-3 py-2'
                      >
                        <span className='font-medium text-xs'>
                          {DIMENSION_LABELS[d.dimension] ?? d.dimension}
                        </span>
                        <div className='flex min-w-0 items-center gap-2'>
                          {d.reason ? (
                            <span className='truncate text-muted-foreground text-xs'>
                              {d.reason}
                            </span>
                          ) : null}
                          <AdminStatusPill tone={workspaceHealthTone(d.status)} label={d.status} />
                        </div>
                      </li>
                    ))}
                  </ul>
                </div>
              </section>

              <section className='space-y-3 rounded-lg border border-border/60 bg-card p-6'>
                <AdminSectionLabel>Signals (recent window)</AdminSectionLabel>
                <dl className='grid grid-cols-1 gap-x-8 gap-y-2 sm:grid-cols-2'>
                  {SIGNAL_ROWS.map((row) => (
                    <div key={row.key} className='flex items-center justify-between gap-3 text-xs'>
                      <dt className='text-muted-foreground'>{row.label}</dt>
                      <dd className='tabular-nums'>
                        {health.signals ? (health.signals[row.key] as number) : "—"}
                      </dd>
                    </div>
                  ))}
                  <div className='flex items-center justify-between gap-3 text-xs'>
                    <dt className='text-muted-foreground'>Latest Airway run</dt>
                    <dd>{health.signals ? airwayLabel(health.signals) : "—"}</dd>
                  </div>
                </dl>
                <p className='text-[11px] text-muted-foreground'>
                  Anomaly counts are informational — they do not affect the health status. Severity
                  measures how far a value sits past its seasonal band, so a campaign or a holiday
                  scores high while nothing is broken. Data-correctness failures surface under
                  Reconciliation.
                </p>
              </section>

              {health.reconciliation.length > 0 ? (
                <section className='space-y-3 rounded-lg border border-border/60 bg-card p-6'>
                  <AdminSectionLabel>Reconciliation</AdminSectionLabel>
                  <ul className='space-y-1.5'>
                    {health.reconciliation.map((check) => (
                      <ReconciliationRow key={check.check} check={check} />
                    ))}
                  </ul>
                </section>
              ) : null}

              {health.smoke.length > 0 || health.smoke_probes.length > 0 ? (
                <SmokeTestSection
                  checks={health.smoke}
                  probes={health.smoke_probes}
                  lastRunAt={health.last_smoke_at}
                  // Both buttons drive the same eval pass; only this one forces the
                  // probes past their cadence, so they share `isPending` and neither
                  // can be double-fired while the other is in flight.
                  onRun={() => triggerEval.mutate({ workspaceId, smoke: true })}
                  isRunning={triggerEval.isPending}
                />
              ) : null}
            </div>
          )
        }
      </AdminAsync>
    </AdminDetailTabPanel>
  );
}

/**
 * One reconciliation check: the actual operand against the expected reference,
 * the absolute and percent drift, an optional description + reason, and its
 * drift status pill. The value chips use the backend-resolved operand labels.
 *
 * The compared window is shown alongside the numbers because drift is only
 * interpretable against the period it was measured over — a check whose
 * `freshness` watermark is too small silently compares days the warehouse has
 * not finished loading, which reads as real drift.
 */
const ReconciliationRow = ({ check }: { check: WorkspaceHealthReconciliationCheck }) => {
  const windowLabel = formatWindow(check);

  return (
    <li className='flex items-center justify-between gap-3 rounded-md border border-border/50 px-3 py-2'>
      <div className='min-w-0 space-y-1'>
        <span className='font-medium text-xs'>{check.check}</span>
        {check.description ? (
          <span className='block truncate text-muted-foreground text-xs'>{check.description}</span>
        ) : null}
        <div className='flex flex-wrap items-center gap-x-3 gap-y-0.5 text-muted-foreground text-xs tabular-nums'>
          <span>
            {check.actual_label} {formatNumber(check.actual)}
          </span>
          <span>
            {check.expected_label} {formatNumber(check.expected)}
          </span>
          <span>
            Δ {formatNumber(check.abs_diff)}
            {check.pct_diff !== null ? ` (${check.pct_diff.toFixed(1)}%)` : null}
          </span>
          {windowLabel ? <span>{windowLabel}</span> : null}
        </div>
        {check.reason ? (
          <span className='block truncate text-muted-foreground text-xs'>{check.reason}</span>
        ) : null}
      </div>
      <AdminStatusPill tone={workspaceHealthTone(check.status)} label={check.status} />
    </li>
  );
};

/**
 * The compared window as `start → end`, with the resolving calendar appended
 * when it isn't UTC. Empty for rows stored before the window was recorded, so
 * they render without a stray separator.
 */
const formatWindow = (check: WorkspaceHealthReconciliationCheck): string => {
  if (!check.window_start || !check.window_end) return "";
  const range = `${check.window_start} → ${check.window_end}`;
  return check.window_timezone && check.window_timezone !== "UTC"
    ? `${range} (${check.window_timezone})`
    : range;
};

/** `null` (source unreachable/errored) and non-finite values render as an em dash. */
const formatNumber = (value: number | null): string => {
  if (value === null || !Number.isFinite(value)) return "—";
  return Number.isInteger(value)
    ? value.toLocaleString()
    : value.toLocaleString(undefined, { maximumFractionDigits: 2 });
};

const airwayLabel = (s: WorkspaceHealthSignals): string => {
  if (s.airway_last_run_failed) return "Failed";
  if (s.airway_completed_with_errors) return "Completed with errors";
  return "OK";
};
