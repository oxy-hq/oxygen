import { AppWindow, ArrowRight, Bot, Check, ChevronDown, Eye, Network } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import useApps from "@/hooks/api/apps/useApps";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import { markOnboardingDismissedForStorageKey } from "@/libs/utils/onboardingStorage";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { appDisplayLabel } from "@/utils/appLabel";
import type { Milestone, OnboardingMode, PhaseTimings } from "../types";

const OVERVIEW_APP_PATH = "apps/overview.app.yml";

/** Order apps so the overview dashboard renders first, then alphabetically by path. */
function sortAppsForExplore<T extends { path: string }>(apps: T[]): T[] {
  return [...apps].sort((a, b) => {
    if (a.path === OVERVIEW_APP_PATH) return -1;
    if (b.path === OVERVIEW_APP_PATH) return 1;
    return a.path.localeCompare(b.path);
  });
}

interface CompletionCardProps {
  sampleQuestions: string[];
  createdFiles: string[];
  agentPath?: string;
  warehouseType?: string;
  milestones?: Milestone[];
  phaseTimings?: PhaseTimings;
  buildDurationMs?: number;
  fileCount?: number;
  /** When `"github"`, the completion card reads apps from the workspace
   *  (via `useApps`) rather than `createdFiles`, since nothing was built. */
  mode?: OnboardingMode;
}

export default function CompletionCard({
  sampleQuestions,
  createdFiles,
  agentPath,
  warehouseType,
  milestones,
  phaseTimings,
  buildDurationMs = 0,
  fileCount = 0,
  mode
}: CompletionCardProps) {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { project } = useCurrentProjectBranch();
  const projectId = project?.id ?? "00000000-0000-0000-0000-000000000000";
  const routes = ROUTES.ORG(orgSlug).WORKSPACE(projectId);

  // Reaching the completion card means onboarding has been dealt with — built
  // or connected, possibly with credentials intentionally skipped. Persist that
  // per workspace so Home's setup-gap check stops bouncing the user back into
  // the wizard on a still-missing key. We mark it here (rather than relying on
  // the per-step "Skip for now") because the credential-driven Home → onboarding
  // redirect never seeds the orchestrator's storageKey, whereas `project` is
  // reliably available in this component.
  const onboardingStorageKey = project?.storage_key ?? project?.id;
  useEffect(() => {
    if (onboardingStorageKey) markOnboardingDismissedForStorageKey(onboardingStorageKey);
  }, [onboardingStorageKey]);

  // The workspace app list: in github mode it is the only source of app paths
  // (nothing is built).
  const { data: workspaceApps } = useApps(true, false, false);
  // Both `github` and `demo` start from a config.yml that ships agents +
  // dashboards already on disk — no semantic-model build, no warehouse
  // summary. They only differ in the ready-state copy.
  const isGithub = mode === "github" || mode === "demo";
  const isDemo = mode === "demo";

  const viewFiles = createdFiles.filter((f) => f.endsWith(".view.yml"));

  // Onboarding never builds an app any more (Data Apps are retired), so only a
  // repository that was imported with apps already in it has any to open.
  const exploreApps = isGithub ? (workspaceApps ?? []) : [];
  const sortedExploreApps = sortAppsForExplore(exploreApps);
  const promptCount = Math.min(3, sampleQuestions.length);

  const warehouseName = warehouseType
    ? warehouseType.charAt(0).toUpperCase() + warehouseType.slice(1)
    : "your warehouse";

  const summaryParts: string[] = [];
  if (viewFiles.length > 0)
    summaryParts.push(`${viewFiles.length} semantic view${viewFiles.length > 1 ? "s" : ""}`);
  summaryParts.push("1 analytics agent");
  const summaryList = summaryParts.join(", ").replace(/, ([^,]*)$/, ", and $1");

  return (
    <div className='flex flex-col gap-6'>
      {/* Setup summary — sits right after the onboarding thread history,
          before the launch block, so it reads as a natural continuation. */}
      {milestones && milestones.length > 0 && !isGithub && (
        <InlineSetupSummary
          milestones={milestones}
          phaseTimings={phaseTimings}
          buildDurationMs={buildDurationMs}
          fileCount={fileCount}
        />
      )}

      {/* Primary: heading + summary + CTA */}
      <div className='flex flex-col gap-3'>
        <p className='font-medium text-sm'>Workspace ready</p>
        <p className='text-muted-foreground text-sm leading-relaxed'>
          {isDemo
            ? "Your demo workspace is set up. Try the sample agents on the bundled DuckDB data."
            : isGithub
              ? "Your repository is connected. You can start asking questions against the agents and dashboards that ship with it."
              : `Connected ${warehouseName} and built ${summaryList}.`}
        </p>
        <Button
          className='w-full justify-center gap-2'
          onClick={() => {
            navigate(routes.HOME, {
              state: { agentPath, autoFocus: true }
            });
          }}
        >
          <Bot className='h-4 w-4' />
          Start asking questions
        </Button>
      </div>

      {/* Explore: dashboards prominent, secondary items behind "See more" */}
      <ExploreSection apps={sortedExploreApps} routes={routes} navigate={navigate} />

      {/* Sample prompts — plain text, clearly secondary */}
      {promptCount > 0 && (
        <div className='flex flex-col gap-2'>
          <p className='text-muted-foreground text-xs uppercase tracking-wider'>
            Try these with your analytics agent
          </p>
          <div className='flex flex-col gap-0.5'>
            {sampleQuestions.slice(0, 3).map((question, index) => (
              <button
                key={question}
                type='button'
                data-testid={`onboarding-sample-prompt-${index}`}
                onClick={() => {
                  navigate(routes.HOME, {
                    state: {
                      prefillQuestion: question,
                      agentPath,
                      autoSubmit: true
                    }
                  });
                }}
                className='flex items-start gap-2 rounded-md px-1 py-1 text-left text-muted-foreground text-sm transition-colors hover:bg-muted/40 hover:text-foreground'
              >
                <span className='mt-1.5 h-1 w-1 shrink-0 rounded-full bg-primary/60' />
                <span className='leading-relaxed'>{question}</span>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// ── Explore section with "See more" disclosure ─────────────────────────────

function ExploreSection({
  apps,
  routes,
  navigate
}: {
  apps: Array<{ path: string; name: string; title?: string }>;
  routes: ReturnType<ReturnType<(typeof ROUTES)["ORG"]>["WORKSPACE"]>;
  navigate: ReturnType<typeof useNavigate>;
}) {
  const [showMore, setShowMore] = useState(false);

  return (
    <div className='flex flex-col gap-2'>
      <p className='text-muted-foreground text-xs uppercase tracking-wider'>Explore</p>
      <div className='grid grid-cols-1 gap-1'>
        {apps.map((app, index) => {
          const label = appDisplayLabel(app);
          return (
            <button
              key={app.path}
              type='button'
              data-testid={`onboarding-explore-app-${index}`}
              onClick={() => {
                const pathb64 = encodeBase64(app.path);
                navigate(routes.APP(pathb64));
              }}
              className={cn(
                "flex items-center gap-2.5 rounded-md border border-border px-3 py-1.5 text-left text-sm",
                "transition-colors hover:border-primary/50 hover:bg-primary/5"
              )}
            >
              <AppWindow className='h-3.5 w-3.5 text-primary' />
              <span>{label}</span>
              <ArrowRight className='ml-auto h-3 w-3 text-muted-foreground' />
            </button>
          );
        })}
      </div>

      {!showMore ? (
        <button
          type='button'
          onClick={() => setShowMore(true)}
          className='text-left text-muted-foreground text-xs transition-colors hover:text-foreground'
        >
          See more &rarr;
        </button>
      ) : (
        <div className='grid grid-cols-1 gap-1'>
          <button
            type='button'
            onClick={() => navigate(routes.IDE.ROOT)}
            className={cn(
              "flex items-center gap-2.5 rounded-md border border-border px-3 py-1.5 text-left text-sm",
              "transition-colors hover:border-primary/50 hover:bg-primary/5"
            )}
          >
            <Eye className='h-3.5 w-3.5 text-primary' />
            <span>Semantic Model</span>
            <ArrowRight className='ml-auto h-3 w-3 text-muted-foreground' />
          </button>
          <button
            type='button'
            onClick={() => navigate(routes.CONTEXT_GRAPH)}
            className={cn(
              "flex items-center gap-2.5 rounded-md border border-border px-3 py-1.5 text-left text-sm",
              "transition-colors hover:border-primary/50 hover:bg-primary/5"
            )}
          >
            <Network className='h-3.5 w-3.5 text-primary' />
            <span>Context Graph</span>
            <ArrowRight className='ml-auto h-3 w-3 text-muted-foreground' />
          </button>
        </div>
      )}
    </div>
  );
}

// ── Inline setup summary ───────────────────────────────────────────────────

function InlineSetupSummary({
  milestones,
  phaseTimings,
  buildDurationMs,
  fileCount
}: {
  milestones: Milestone[];
  phaseTimings?: PhaseTimings;
  buildDurationMs: number;
  fileCount: number;
}) {
  const [open, setOpen] = useState(false);

  return (
    <div>
      <button
        type='button'
        onClick={() => setOpen((v) => !v)}
        className='flex w-full items-center gap-2 text-left'
      >
        <ChevronDown
          className={cn(
            "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
            !open && "-rotate-90"
          )}
        />
        <span className='text-muted-foreground text-xs uppercase tracking-wider'>
          Setup Summary
        </span>
      </button>

      {open && (
        <ol className='mt-2 flex flex-col gap-1'>
          {milestones.map((m) => (
            <SummaryMilestone
              key={m.id}
              milestone={m}
              phaseTimings={phaseTimings}
              buildDurationMs={buildDurationMs}
              fileCount={fileCount}
            />
          ))}
        </ol>
      )}
    </div>
  );
}

function SummaryMilestone({
  milestone,
  phaseTimings,
  buildDurationMs,
  fileCount
}: {
  milestone: Milestone;
  phaseTimings?: PhaseTimings;
  buildDurationMs: number;
  fileCount: number;
}) {
  const { label, detail, children } = milestone;
  const isBuild = milestone.id === "build";
  const rowDetail = isBuild && buildDurationMs > 0 ? formatDuration(buildDurationMs) : detail;

  return (
    <li className='flex flex-col gap-0.5'>
      <div className='flex items-center gap-2'>
        <Check className='h-3 w-3 shrink-0 text-primary' />
        <span className='text-muted-foreground text-xs'>{label}</span>
        {rowDetail && (
          <span className='ml-auto text-muted-foreground/60 text-xs tabular-nums'>{rowDetail}</span>
        )}
      </div>
      {children && children.length > 0 && (
        <div className='ml-5 flex flex-col gap-0.5 border-border/40 border-l pl-3'>
          {children.map((child) => {
            const phaseKey = idToPhaseKey(child.id);
            const duration = phaseKey ? phaseDuration(phaseKey, phaseTimings) : undefined;
            return (
              <div key={child.id} className='flex items-center gap-2'>
                <Check className='h-2.5 w-2.5 shrink-0 text-primary' />
                <span className='text-muted-foreground text-xs'>{child.label}</span>
                {duration != null && (
                  <span className='ml-auto text-muted-foreground/60 text-xs tabular-nums'>
                    {formatSeconds(duration)}
                  </span>
                )}
              </div>
            );
          })}
          {isBuild && fileCount > 0 && (
            <span className='text-muted-foreground/60 text-xs'>
              {fileCount} file{fileCount === 1 ? "" : "s"} created
            </span>
          )}
        </div>
      )}
    </li>
  );
}

// ── Helpers ─────────────────────────────────────────────────────────────────

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

function phaseDuration(key: "semantic" | "agent", timings?: PhaseTimings): number | undefined {
  const t = timings?.[key];
  if (!t?.start || !t.end) return undefined;
  return Math.max(0, Math.round((t.end - t.start) / 1000));
}
