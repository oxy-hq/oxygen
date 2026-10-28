import { Check, ChevronDown, Circle, Loader2 } from "lucide-react";
import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type {
  ExpectedFile,
  GeneratedArtifact,
  Milestone,
  OnboardingRailState,
  PhaseTimings
} from "./types";

interface OnboardingRightRailProps {
  railState: OnboardingRailState;
  buildDurationMs?: number;
  phaseTimings?: PhaseTimings;
}

export default function OnboardingRightRail({
  railState,
  buildDurationMs = 0,
  phaseTimings
}: OnboardingRightRailProps) {
  const isBuildComplete = !!railState.isBuildComplete;
  const isBuilding = railState.isBuildRunning;
  const isInBuildFlow = isBuilding || isBuildComplete;

  const subtitle = isBuildComplete
    ? "Workspace ready"
    : isBuilding
      ? "Building your workspace"
      : "Configuring your workspace";

  return (
    <div
      className={cn(
        "flex flex-col border-border border-l bg-card",
        isBuildComplete ? "h-auto" : "h-full"
      )}
    >
      <div className={cn("border-border border-b px-4", isBuildComplete ? "py-2.5" : "py-3")}>
        <h3 className={cn("font-medium", isBuildComplete ? "text-xs" : "text-sm")}>
          Setup Progress
        </h3>
        <p className='text-muted-foreground text-xs'>{subtitle}</p>
      </div>

      <div className={cn("px-4", isBuildComplete ? "py-3" : "flex-1 overflow-y-auto py-4")}>
        <MilestoneList
          milestones={railState.milestones}
          phaseTimings={phaseTimings}
          artifacts={railState.generatedArtifacts}
          expectedFiles={railState.expectedFiles}
          isBuildComplete={isBuildComplete}
          buildDurationMs={buildDurationMs}
        />

        {!isInBuildFlow && railState.selectedTables.length > 0 && (
          <div className='mt-6'>
            <h4 className='mb-2 text-muted-foreground text-xs uppercase tracking-wider'>
              Selected Tables
            </h4>
            <div className='flex flex-col gap-1'>
              {railState.selectedTables.map((table) => (
                <div
                  key={table}
                  className='truncate font-mono text-muted-foreground text-xs'
                  title={table}
                >
                  {table}
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// ── Milestone list ───────────────────────────────────────────────────────────

interface MilestoneListProps {
  milestones: Milestone[];
  phaseTimings?: PhaseTimings;
  artifacts: GeneratedArtifact[];
  expectedFiles: ExpectedFile[];
  isBuildComplete: boolean;
  buildDurationMs: number;
}

function MilestoneList({
  milestones,
  phaseTimings,
  artifacts,
  expectedFiles,
  isBuildComplete,
  buildDurationMs
}: MilestoneListProps) {
  return (
    <ol className={cn("flex flex-col", isBuildComplete ? "gap-1.5" : "gap-3.5")}>
      {milestones.map((m) => (
        <MilestoneRow
          key={m.id}
          milestone={m}
          phaseTimings={phaseTimings}
          artifacts={artifacts}
          expectedFiles={expectedFiles}
          isBuildComplete={isBuildComplete}
          buildDurationMs={buildDurationMs}
        />
      ))}
    </ol>
  );
}

interface MilestoneRowProps {
  milestone: Milestone;
  phaseTimings?: PhaseTimings;
  artifacts: GeneratedArtifact[];
  expectedFiles: ExpectedFile[];
  isBuildComplete: boolean;
  buildDurationMs: number;
}

function MilestoneRow({
  milestone,
  phaseTimings,
  artifacts,
  expectedFiles,
  isBuildComplete,
  buildDurationMs
}: MilestoneRowProps) {
  const { status, label, detail, children } = milestone;
  const isBuildMilestone = milestone.id === "build";

  const rowDetail = isBuildMilestone
    ? buildDurationMs > 0
      ? formatDuration(buildDurationMs)
      : detail
    : detail;

  return (
    <li className={cn("flex flex-col", isBuildComplete ? "gap-0.5" : "gap-1.5")}>
      <div className='flex items-center gap-2'>
        <MilestoneIcon status={status} size={isBuildComplete ? "sm" : "md"} />
        <p
          className={cn(
            isBuildComplete ? "text-xs" : "text-sm",
            status === "pending" && "text-muted-foreground/50",
            status === "active" && "font-medium text-foreground",
            status === "complete" &&
              (isBuildComplete ? "text-muted-foreground" : "text-foreground"),
            status === "error" && "text-destructive"
          )}
        >
          {label}
        </p>
        {rowDetail && (
          <span
            className={cn(
              "ml-auto truncate tabular-nums",
              isBuildComplete ? "text-muted-foreground/60 text-xs" : "text-muted-foreground text-xs"
            )}
            title={rowDetail}
          >
            {rowDetail}
          </span>
        )}
      </div>

      {status === "active" && !children && <ActiveProgressBar />}

      {children && children.length > 0 && (
        <div
          className={cn(
            "ml-2 flex flex-col border-border/60 border-l pl-3",
            isBuildComplete ? "mt-0.5 gap-0.5" : "mt-1 gap-2"
          )}
        >
          <ol className={cn("flex flex-col", isBuildComplete ? "gap-0.5" : "gap-2")}>
            {children.map((child) => (
              <SubPhaseRow
                key={child.id}
                milestone={child}
                phaseTimings={phaseTimings}
                compact={isBuildComplete}
              />
            ))}
          </ol>

          {isBuildMilestone &&
            expectedFiles.length > 0 &&
            (isBuildComplete ? (
              <p className='text-muted-foreground/60 text-xs'>
                {expectedFiles.length} file{expectedFiles.length === 1 ? "" : "s"} created
              </p>
            ) : (
              <CreatedFilesSection artifacts={artifacts} expectedFiles={expectedFiles} />
            ))}
        </div>
      )}
    </li>
  );
}

function SubPhaseRow({
  milestone,
  phaseTimings,
  compact = false
}: {
  milestone: Milestone;
  phaseTimings?: PhaseTimings;
  compact?: boolean;
}) {
  const { status, label, id } = milestone;
  const phaseKey = idToPhaseKey(id);
  const duration = phaseKey ? computePhaseDurationSeconds(phaseKey, phaseTimings) : undefined;

  return (
    <li className='flex items-center gap-2'>
      <MilestoneIcon status={status} size='sm' />
      <p
        className={cn(
          "text-xs",
          status === "pending" && "text-muted-foreground/50",
          status === "active" && "font-medium text-foreground",
          status === "complete" && (compact ? "text-muted-foreground" : "text-foreground"),
          status === "error" && "text-destructive"
        )}
      >
        {label}
      </p>
      {duration != null && (status === "complete" || status === "error") && (
        <span
          className={cn(
            "ml-auto text-xs tabular-nums",
            compact ? "text-muted-foreground/60" : "text-muted-foreground"
          )}
        >
          {formatSeconds(duration)}
        </span>
      )}
    </li>
  );
}

function ActiveProgressBar() {
  return (
    <div className='ml-6 h-0.5 overflow-hidden rounded-full bg-muted'>
      <div className='h-full w-full animate-pulse bg-primary/50' />
    </div>
  );
}

function MilestoneIcon({
  status,
  size = "md"
}: {
  status: Milestone["status"];
  size?: "sm" | "md";
}) {
  const wrap = size === "sm" ? "h-3.5 w-3.5" : "h-5 w-5";
  const icon = size === "sm" ? "h-2 w-2" : "h-3 w-3";
  const spin = size === "sm" ? "h-2.5 w-2.5" : "h-3.5 w-3.5";

  if (status === "complete") {
    return (
      <div
        className={cn("flex shrink-0 items-center justify-center rounded-full bg-primary/10", wrap)}
      >
        <Check className={cn(icon, "text-primary")} />
      </div>
    );
  }
  if (status === "active") {
    return (
      <div className={cn("flex shrink-0 items-center justify-center", wrap)}>
        <Loader2 className={cn(spin, "animate-spin text-primary")} />
      </div>
    );
  }
  if (status === "error") {
    return (
      <div
        className={cn(
          "flex shrink-0 items-center justify-center rounded-full bg-destructive/10",
          wrap
        )}
      >
        <Circle className={cn("fill-destructive text-destructive", icon)} />
      </div>
    );
  }
  return (
    <div className={cn("flex shrink-0 items-center justify-center", wrap)}>
      <Circle
        className={cn(size === "sm" ? "h-1.5 w-1.5" : "h-2 w-2", "text-muted-foreground/30")}
      />
    </div>
  );
}

// ── Created files disclosure (building only) ─────────────────────────────────

function CreatedFilesSection({
  artifacts,
  expectedFiles
}: {
  artifacts: GeneratedArtifact[];
  expectedFiles: ExpectedFile[];
}) {
  const [open, setOpen] = useState(true);
  const total = expectedFiles.length;

  const createdNames = new Set(
    artifacts.map(
      (a) =>
        a.filePath
          .split("/")
          .pop()
          ?.replace(/\.(view|topic)\.yml$/, "") ?? a.filePath
    )
  );
  const isExpectedDone = (f: ExpectedFile): boolean => createdNames.has(f.name);
  const completed = expectedFiles.filter(isExpectedDone).length;
  const firstPendingIdx = expectedFiles.findIndex((f) => !isExpectedDone(f));

  return (
    <div className='mt-1'>
      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        className='flex w-full items-center gap-2 py-0.5 text-left'
      >
        <ChevronDown
          className={cn(
            "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
            !open && "-rotate-90"
          )}
        />
        <h4 className='text-muted-foreground text-xs uppercase tracking-wider'>Creating Files</h4>
        <span className='ml-auto text-muted-foreground text-xs tabular-nums'>
          {completed} / {total}
        </span>
      </button>

      {open && (
        <div className='mt-2 flex flex-col gap-1.5 pl-5'>
          {expectedFiles.map((expected, i) => {
            const isDone = isExpectedDone(expected);
            const isActive = !isDone && i === firstPendingIdx;
            // Key includes type + index because tables from different schemas
            // can share the same short name, which would otherwise collide.
            return (
              <div
                key={`${expected.type}:${expected.name}:${i}`}
                className='flex items-center gap-2 truncate'
              >
                {isDone ? (
                  <Check className='h-3 w-3 shrink-0 text-primary' />
                ) : isActive ? (
                  <Loader2 className='h-3 w-3 shrink-0 animate-spin text-primary/50' />
                ) : (
                  <Circle className='h-1.5 w-1.5 shrink-0 text-muted-foreground/30' />
                )}
                <span
                  className={cn(
                    "truncate text-xs",
                    isDone ? "text-foreground" : "text-muted-foreground/50"
                  )}
                >
                  {expected.name}
                </span>
                <span className='shrink-0 text-muted-foreground text-xs'>{expected.type}</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function formatSeconds(s: number): string {
  if (s < 60) return `${s}s`;
  const mins = Math.floor(s / 60);
  const secs = s % 60;
  if (secs === 0) return `${mins}m`;
  return `${mins}m ${secs}s`;
}

function formatDuration(ms: number): string {
  return formatSeconds(Math.round(ms / 1000));
}

function idToPhaseKey(id: string): "semantic" | "agent" | null {
  if (id === "build-semantic") return "semantic";
  if (id === "build-agent") return "agent";
  return null;
}

function computePhaseDurationSeconds(
  key: "semantic" | "agent",
  timings?: PhaseTimings
): number | undefined {
  const timing = timings?.[key];
  if (!timing?.start || !timing.end) return undefined;
  return Math.max(0, Math.round((timing.end - timing.start) / 1000));
}
