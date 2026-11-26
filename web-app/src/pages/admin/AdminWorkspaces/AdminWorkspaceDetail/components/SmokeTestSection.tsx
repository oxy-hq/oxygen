import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { timeAgo } from "@/libs/utils/date";
import type {
  WorkspaceHealthSmokeCheck,
  WorkspaceHealthSmokeProbe,
  WorkspaceHealthSmokeProbeKind,
  WorkspaceHealthStatus
} from "@/services/api/workspaceHealth";
import { AdminSectionLabel } from "../../../components/AdminSectionLabel";
import { AdminStatusPill } from "../../../components/AdminStatusPill";
import { workspaceHealthTone } from "../../../components/workspaceHealthTone";

const KIND_LABELS: Record<WorkspaceHealthSmokeProbeKind, string> = {
  connection: "Connections",
  semantic: "Semantic model",
  app: "Data apps",
  agent: "Agent"
};

/** Cheapest probe first — the same order the backend runs and documents them in. */
const KIND_ORDER: WorkspaceHealthSmokeProbeKind[] = ["connection", "semantic", "app", "agent"];

/** Worst first, so a single failure never hides below a wall of passing rows. */
const STATUS_RANK: Record<WorkspaceHealthStatus, number> = {
  unhealthy: 0,
  degraded: 1,
  healthy: 2
};

/**
 * A `healthy` check carrying a reason is a *note*, not a probe result — the
 * backend emits those to record targets dropped by the `max_targets` cap. Its
 * `passed()` constructor never sets a reason, so this predicate is exact.
 */
const isNote = (check: WorkspaceHealthSmokeCheck): boolean =>
  check.status === "healthy" && check.reason !== null;

/** Sub-second probes read better in ms; anything slower in seconds. */
const formatDuration = (ms: number): string =>
  ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;

interface SmokeGroup {
  kind: WorkspaceHealthSmokeProbeKind;
  enabled: boolean;
  checks: WorkspaceHealthSmokeCheck[];
}

/**
 * One row per probe kind, in cost order, each carrying its enabled state and its
 * checks. Driven by `probes` (the backend lists all four kinds) so a disabled
 * probe is shown as such rather than dropped. Falls back to inferring enablement
 * from the checks themselves for rows written before `smoke_probes` existed.
 */
const buildGroups = (
  checks: WorkspaceHealthSmokeCheck[],
  probes: WorkspaceHealthSmokeProbe[]
): SmokeGroup[] => {
  const checksFor = (kind: WorkspaceHealthSmokeProbeKind) =>
    checks
      .filter((c) => c.kind === kind)
      .sort(
        (a, b) => STATUS_RANK[a.status] - STATUS_RANK[b.status] || a.target.localeCompare(b.target)
      );

  if (probes.length === 0) {
    // Back-compat: no enablement info, so show only kinds that produced checks
    // and treat them as enabled — the pre-`smoke_probes` behaviour.
    return KIND_ORDER.map((kind) => ({ kind, enabled: true, checks: checksFor(kind) })).filter(
      (g) => g.checks.length > 0
    );
  }

  const enabledByKind = new Map(probes.map((p) => [p.kind, p.enabled]));
  return KIND_ORDER.map((kind) => ({
    kind,
    enabled: enabledByKind.get(kind) ?? false,
    checks: checksFor(kind)
  }));
};

const summarise = (checks: WorkspaceHealthSmokeCheck[]): string => {
  const notes = checks.filter(isNote);
  const probes = checks.filter((c) => !isNote(c));
  const parts = [
    `${probes.filter((c) => c.status === "healthy").length} passed`,
    `${probes.filter((c) => c.status === "unhealthy").length} failed`,
    `${probes.filter((c) => c.status === "degraded").length} degraded`
  ];
  if (notes.length > 0) parts.push(`${notes.length} skipped`);
  return parts.join(" · ");
};

/**
 * The workspace smoke test: every probe kind, shown whether or not it ran.
 * Enabled kinds show their checks (worst-first); disabled kinds are named
 * explicitly so "nothing here" reads as "off", not "unknown".
 *
 * The probes run on their own slower cadence than the health rollup, so this
 * section shows its own `last_smoke_at` rather than the rollup's "last checked"
 * — reading a 6-hour-old probe result as if it were minutes old would be worse
 * than showing no timestamp at all.
 */
export const SmokeTestSection = ({
  checks,
  probes,
  lastRunAt,
  onRun,
  isRunning = false
}: {
  checks: WorkspaceHealthSmokeCheck[];
  probes: WorkspaceHealthSmokeProbe[];
  lastRunAt: string | null;
  /**
   * Run the probes now, out of cadence. Omit to render the section read-only —
   * the probes then only run on their own (default 6h) schedule.
   */
  onRun?: () => void;
  isRunning?: boolean;
}) => {
  const groups = buildGroups(checks, probes);

  return (
    <section className='space-y-4 border border-border/60 bg-card p-6'>
      <AdminSectionLabel
        trailing={
          <span className='flex items-center gap-3'>
            {lastRunAt ? (
              <span title={new Date(lastRunAt).toLocaleString()}>
                Last run {timeAgo(lastRunAt)}
              </span>
            ) : null}
            {onRun ? (
              <Button
                variant='outline'
                size='sm'
                className='gap-1.5'
                onClick={onRun}
                disabled={isRunning}
                // The probes hit the warehouse and (when configured) spend agent
                // tokens, so say so before the click rather than after.
                title='Re-run every enabled probe now, ignoring the smoke cadence'
              >
                <RefreshCw className={isRunning ? "size-3.5 animate-spin" : "size-3.5"} />
                {isRunning ? "Running…" : "Run smoke test"}
              </Button>
            ) : null}
          </span>
        }
      >
        Smoke test
      </AdminSectionLabel>

      <p className='text-muted-foreground text-xs tabular-nums'>{summarise(checks)}</p>

      <div className='space-y-4'>
        {groups.map((group) => (
          <SmokeKindGroup key={group.kind} group={group} lastRunAt={lastRunAt} />
        ))}
      </div>
    </section>
  );
};

const SmokeKindGroup = ({ group, lastRunAt }: { group: SmokeGroup; lastRunAt: string | null }) => {
  const probes = group.checks.filter((c) => !isNote(c));
  const passed = probes.filter((c) => c.status === "healthy").length;

  return (
    <div className='space-y-1.5'>
      <div className='flex items-baseline justify-between gap-3'>
        <span className='font-medium text-xs'>{KIND_LABELS[group.kind]}</span>
        {group.enabled && probes.length > 0 ? (
          <span className='text-muted-foreground text-xs tabular-nums'>
            {passed}/{probes.length} passed
          </span>
        ) : (
          // A disabled probe gets a muted "Not enabled" pill so its empty state
          // reads as a choice, not a gap. An enabled probe with no checks yet
          // gets no pill — the empty-state line below explains it.
          !group.enabled && <AdminStatusPill tone='muted' label='Not enabled' />
        )}
      </div>

      {group.enabled && group.checks.length === 0 ? (
        <p className=' border border-border/50 border-dashed px-3 py-2 text-muted-foreground text-xs'>
          {lastRunAt ? "No targets found" : "Not run yet"}
        </p>
      ) : null}

      {group.checks.length > 0 ? (
        <ul className='space-y-1.5'>
          {group.checks.map((check) => (
            <SmokeCheckRow key={check.check} check={check} />
          ))}
        </ul>
      ) : null}
    </div>
  );
};

/**
 * One probe: the target it exercised, why it failed (or what was skipped), and
 * how long it took. `duration_ms` is 0 for checks that never ran a probe — cap
 * notes and unavailable contexts — so the timing is omitted rather than shown
 * as a misleading "0 ms".
 */
const SmokeCheckRow = ({ check }: { check: WorkspaceHealthSmokeCheck }) => {
  const note = isNote(check);
  return (
    <li className='flex items-center justify-between gap-3 border border-border/50 px-3 py-2'>
      <div className='min-w-0 space-y-1'>
        <span className='font-medium text-xs'>{check.target}</span>
        {check.reason ? (
          <span className='block truncate text-muted-foreground text-xs'>{check.reason}</span>
        ) : null}
        {check.duration_ms > 0 ? (
          <span className='block text-muted-foreground/70 text-xs tabular-nums'>
            {formatDuration(check.duration_ms)}
          </span>
        ) : null}
      </div>
      {/* A cap note isn't a probe outcome, so it gets a muted "skipped" chip
          rather than a green "healthy" one it never earned. */}
      <AdminStatusPill
        tone={note ? "muted" : workspaceHealthTone(check.status)}
        label={note ? "skipped" : check.status}
      />
    </li>
  );
};
