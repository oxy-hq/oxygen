import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Dialog, DialogContent } from "@/components/ui/shadcn/dialog";
import type { SelectableItem } from "@/hooks/analyticsSteps";
import type { UseAnalyticsRunResult } from "@/hooks/useAnalyticsRun";
import type { BuilderActivityItem } from "@/hooks/useBuilderActivity";
import { cn } from "@/libs/shadcn/utils";
import { markOnboardingDismissedForStorageKey } from "@/libs/utils/onboardingStorage";
import AnalyticsArtifactSidebar from "@/pages/thread/analytics/AnalyticsArtifactSidebar";
import type { UiBlock } from "@/services/api/analytics";
import BuildJobsPanel, { type BuildJob, type JobStatus } from "./components/BuildJobsPanel";
import CompletionCard from "./components/CompletionCard";
import CredentialForm from "./components/CredentialForm";
import OnboardingMessage from "./components/OnboardingMessage";
import SecureInput from "./components/SecureInput";
import SelectionCards from "./components/SelectionCards";
import TableSelector from "./components/TableSelector";
import { MODEL_OPTIONS, type useOnboardingOrchestrator } from "./orchestrator";
import type {
  OnboardingInputBlock,
  OnboardingRailState,
  PhaseProgress,
  PhaseTimings,
  SubPhaseKey
} from "./types";
import type { useOnboardingActions } from "./useOnboardingActions";

type Orchestrator = ReturnType<typeof useOnboardingOrchestrator>;
type Actions = ReturnType<typeof useOnboardingActions>;

interface OnboardingThreadProps {
  orchestrator: Orchestrator;
  actions: Actions;
  semanticRun: UseAnalyticsRunResult;
  agentRun: UseAnalyticsRunResult;
  builderActivityItems: BuilderActivityItem[];
  viewEvents?: UiBlock[];
  viewsRunning?: boolean;
  phaseProgress?: Record<SubPhaseKey, PhaseProgress>;
  buildElapsedMs?: number;
  /** Passed only in complete mode (single-column) so CompletionCard can
   *  render the milestone summary inline. */
  railState?: OnboardingRailState;
  phaseTimings?: PhaseTimings;
  /** Parent-supplied retry handler for the "Retry Build" confirm button.
   *  The parent owns the analytics run hooks + view manager, so a simple
   *  `goToStep("building")` from here would leave zombie runs streaming. */
  onRetryBuild?: () => void;
}

// The per-phase messages are replaced by `BuildJobsPanel` in the build step —
// filter them out so we don't render duplicate narrative.
const BUILD_PHASE_MESSAGE_IDS = new Set(["phase_semantic", "phase_agent"]);

/** Message-id prefix for GitHub-mode LLM key prompts. */
const GITHUB_LLM_KEY_PREFIX = "github_llm_key_";
/** Message-id prefix for GitHub-mode warehouse credential prompts. */
const GITHUB_WAREHOUSE_PREFIX = "github_warehouse_";

/**
 * Map a wizard message id to a test-id prefix for the SelectionCards
 * children, so agentic browser tests can target options by id rather than
 * visible text. Returns undefined for messages that don't render selection
 * cards (or for which test stability isn't a concern).
 */
function selectionCardsTestIdPrefix(messageId: string): string | undefined {
  switch (messageId) {
    case "warehouse_type":
      return "onboarding-warehouse";
    case "llm_provider":
      return "onboarding-llm-provider";
    case "llm_model":
      return "onboarding-llm-model";
    default:
      return undefined;
  }
}

/** Advance past a skipped prompt in GitHub mode by dispatching the cursor move. */
function handleSkip(messageId: string, orchestrator: Orchestrator) {
  // Remember that the user deferred setup for this workspace. Without this,
  // Home re-redirects into the wizard on every visit — skipping advances the
  // step but never satisfies the missing-credential check that drives the
  // redirect, so the user gets bounced straight back. The gap still shows as a
  // dismissible row on Home and the wizard stays reachable from there.
  if (orchestrator.state.storageKey) {
    markOnboardingDismissedForStorageKey(orchestrator.state.storageKey);
  }
  if (messageId.startsWith(GITHUB_LLM_KEY_PREFIX)) {
    orchestrator.advanceGithubLlmKey();
    return;
  }
  if (messageId.startsWith(GITHUB_WAREHOUSE_PREFIX)) {
    const name = messageId.substring(GITHUB_WAREHOUSE_PREFIX.length);
    orchestrator.advanceGithubWarehouse(name, "skipped");
  }
}

function toUiBlocks(run: UseAnalyticsRunResult): UiBlock[] {
  if (!("events" in run.state)) return [];
  return run.state.events.map((ev) => ({
    seq: Number(ev.id ?? 0),
    event_type: ev.type,
    payload: ev.data
  })) as UiBlock[];
}

function runStatus(
  run: UseAnalyticsRunResult,
  phaseStatus: "running" | "done" | "failed" | undefined,
  hasStarted: boolean
): JobStatus {
  if (phaseStatus === "done") return "done";
  if (phaseStatus === "failed") return "failed";
  if (phaseStatus === "running") return "running";
  if (run.state.tag === "running" || run.state.tag === "suspended") return "running";
  if (run.state.tag === "done") return "done";
  if (run.state.tag === "failed") return "failed";
  return hasStarted ? "running" : "queued";
}

export default function OnboardingThread({
  orchestrator,
  actions,
  semanticRun,
  agentRun,
  builderActivityItems: _builderActivityItems,
  viewEvents = [],
  viewsRunning = false,
  phaseProgress,
  buildElapsedMs = 0,
  railState,
  phaseTimings,
  onRetryBuild
}: OnboardingThreadProps) {
  const { messages, state } = orchestrator;
  const bottomRef = useRef<HTMLDivElement>(null);
  const [selectedArtifact, setSelectedArtifact] = useState<SelectableItem | null>(null);

  const handleSelectArtifact = useCallback((item: SelectableItem) => {
    setSelectedArtifact(item);
  }, []);

  const semanticEvents = toUiBlocks(semanticRun);
  const agentEvents = toUiBlocks(agentRun);

  const semanticCombinedEvents = useMemo(
    () => [...semanticEvents, ...viewEvents],
    [semanticEvents, viewEvents]
  );

  // Semantic job combines config + all parallel view runs, so its "running" state
  // is the union of config streaming and any view actively building.
  const semanticStatus = useMemo<JobStatus>(() => {
    const ps = state.phaseStatuses ?? {};
    const totalViews = state.selectedTables.length;
    const viewValues = Object.values(state.viewRunStatuses ?? {});
    const doneViews = viewValues.filter((s) => s === "done" || s === "failed").length;
    const anyFailed = ps.config === "failed" || viewValues.includes("failed");
    const allDone = ps.config === "done" && totalViews > 0 && doneViews === totalViews;
    if (allDone && !anyFailed) return "done";
    if (allDone && anyFailed) return "failed";
    if (ps.config === "running" || viewsRunning) return "running";
    if (!ps.config && totalViews === 0) return "queued";
    return "running";
  }, [state.phaseStatuses, state.viewRunStatuses, state.selectedTables.length, viewsRunning]);

  const jobs = useMemo<BuildJob[]>(() => {
    const ps = state.phaseStatuses ?? {};
    const totalViews = state.selectedTables.length;
    const doneViews = Object.values(state.viewRunStatuses ?? {}).filter(
      (s) => s === "done" || s === "failed"
    ).length;

    const semanticBadge =
      totalViews > 0 && semanticStatus === "running"
        ? `${doneViews} of ${totalViews} views`
        : undefined;

    const semanticProgress = phaseProgress?.semantic ?? {
      ratio: 0,
      elapsedSeconds: 0,
      estimatedSeconds: 0,
      isRunning: false
    };
    const agentProgress = phaseProgress?.agent ?? semanticProgress;

    const jobs: BuildJob[] = [
      {
        id: "semantic",
        label: "Semantic model",
        status: semanticStatus,
        events: semanticCombinedEvents,
        run: undefined,
        progress: semanticProgress,
        badge: semanticBadge
      },
      {
        id: "agent",
        label: "Analytics agent",
        status: runStatus(agentRun, ps.agent, !!ps.agent),
        events: agentEvents,
        run: agentRun,
        progress: agentProgress
      }
    ];

    return jobs;
  }, [
    state.phaseStatuses,
    state.viewRunStatuses,
    state.selectedTables,
    state.selectedTables.length,
    semanticStatus,
    semanticCombinedEvents,
    agentRun,
    agentEvents,
    phaseProgress
  ]);

  const isBuildPhase = state.step === "building" || state.step === "complete";
  const visibleMessages = useMemo(
    () => (isBuildPhase ? messages.filter((m) => !BUILD_PHASE_MESSAGE_IDS.has(m.id)) : messages),
    [messages, isBuildPhase]
  );

  const anyStreaming = [semanticRun, agentRun].some(
    (r) => r.state.tag === "running" || r.state.tag === "suspended"
  );
  const totalEventCount = semanticCombinedEvents.length + agentEvents.length;

  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll should trigger when messages/events change
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [visibleMessages.length, state.step, totalEventCount, anyStreaming]);

  return (
    <div className='flex h-full flex-col'>
      <div className='customScrollbar flex-1 overflow-y-auto [scrollbar-gutter:stable_both-edges]'>
        <div
          className={cn(
            "mx-auto flex w-full flex-col gap-4 px-4 py-8",
            state.step === "complete" ? "max-w-3xl" : "max-w-2xl"
          )}
        >
          {visibleMessages.map((message, i) => (
            <div key={message.id} className='flex flex-col gap-1'>
              <OnboardingMessage
                message={message}
                isLatest={!isBuildPhase && i === visibleMessages.length - 1}
              >
                <div className='flex flex-col gap-3'>
                  {renderInputBlock(message, orchestrator, actions, onRetryBuild)}
                  {message.allowSkip && message.inputBlock && (
                    <button
                      type='button'
                      onClick={() => handleSkip(message.id, orchestrator)}
                      className='self-start text-muted-foreground text-xs hover:text-foreground'
                    >
                      Skip for now &rarr;
                    </button>
                  )}
                  {message.goBackStep && (
                    <button
                      type='button'
                      onClick={() => {
                        if (message.goBackStep) orchestrator.goToStep(message.goBackStep);
                      }}
                      className='self-start text-muted-foreground text-xs hover:text-foreground'
                    >
                      &larr; {message.goBackLabel ?? "Go back"}
                    </button>
                  )}
                </div>
              </OnboardingMessage>
            </div>
          ))}

          {/* Orchestration panel — live build only. On complete the panel
              has served its purpose; CompletionCard owns the finished state. */}
          {state.step === "building" && !state.buildError && (
            <BuildJobsPanel
              jobs={jobs}
              isComplete={false}
              totalElapsedMs={buildElapsedMs}
              onSelectArtifact={handleSelectArtifact}
              isAutoAcceptable={isAutoAcceptableQuestion}
            />
          )}

          {/* Completion — setup summary sits between the thread history and
              the launch block so it reads as a natural continuation of the
              onboarding steps, not as part of the CTA surface. */}
          {state.step === "complete" &&
            (() => {
              const files = state.createdFiles ?? [];
              const agentFile = files.find(
                (f) => f.endsWith(".agentic.yml") || f.endsWith(".agentic.yaml")
              );
              const questions = state.sampleQuestions ?? [];
              return (
                <CompletionCard
                  sampleQuestions={questions}
                  createdFiles={files}
                  agentPath={agentFile}
                  warehouseType={state.warehouseType}
                  milestones={railState?.milestones}
                  phaseTimings={phaseTimings}
                  buildDurationMs={buildElapsedMs}
                  fileCount={railState?.expectedFiles.length ?? 0}
                  mode={state.mode}
                />
              );
            })()}

          <div ref={bottomRef} />
        </div>
      </div>

      {/* Artifact preview modal */}
      <Dialog open={!!selectedArtifact} onOpenChange={(open) => !open && setSelectedArtifact(null)}>
        <DialogContent
          className='flex h-[70vh] max-w-2xl flex-col overflow-hidden p-0'
          showCloseButton={false}
        >
          {selectedArtifact && selectedArtifact.kind !== "builder_delegation" && (
            <AnalyticsArtifactSidebar
              item={selectedArtifact}
              onClose={() => setSelectedArtifact(null)}
            />
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

function renderInputBlock(
  message: {
    id: string;
    inputBlock?: OnboardingInputBlock;
    status?: string;
  },
  orchestrator: Orchestrator,
  actions: Actions,
  onRetryBuild?: () => void
) {
  const block = message.inputBlock;
  if (!block) return null;

  switch (block.type) {
    case "selection_cards":
      return (
        <SelectionCards
          options={block.options}
          collapseAfter={block.collapseAfter}
          testIdPrefix={selectionCardsTestIdPrefix(message.id)}
          onSelect={(id) => {
            if (message.id === "llm_provider") {
              orchestrator.setLlmProvider(id as Parameters<typeof orchestrator.setLlmProvider>[0]);
            } else if (message.id === "llm_model") {
              const provider = orchestrator.state.llmProvider ?? "anthropic";
              const modelOpt = MODEL_OPTIONS[provider]?.find((m) => m.id === id);
              if (modelOpt) {
                orchestrator.setLlmModel(modelOpt.id, modelOpt.modelRef, modelOpt.vendor);
              }
            } else if (message.id === "warehouse_type") {
              orchestrator.setWarehouseType(
                id as Parameters<typeof orchestrator.setWarehouseType>[0]
              );
            }
          }}
        />
      );

    case "secure_input":
      return (
        <SecureInput
          label={block.label}
          placeholder={block.placeholder}
          buttonLabel={block.buttonLabel}
          disabled={block.busy}
          errorMessage={block.errorMessage}
          // The save actions cannot reject: each catches its own request
          // failures and reports them on the step (inline error) or by toast.
          onSubmit={(value) => {
            if (message.id === "llm_key") {
              void actions.saveLlmKey(value);
            } else if (message.id.startsWith(GITHUB_LLM_KEY_PREFIX)) {
              const varName = message.id.substring(GITHUB_LLM_KEY_PREFIX.length);
              void actions.saveGithubLlmKey(varName, value);
            }
          }}
        />
      );

    case "credential_form":
      return (
        <CredentialForm
          fields={block.fields}
          buttonLabel={block.buttonLabel}
          initialValues={block.initialValues}
          initialUploadedFiles={block.initialUploadedFiles}
          disabled={block.busy}
          errorMessage={block.errorMessage}
          // Neither action can reject: a failed save, test or sync is caught
          // there and lands on the step as its connection error.
          onSubmit={(values) => {
            if (message.id.startsWith(GITHUB_WAREHOUSE_PREFIX)) {
              const name = message.id.substring(GITHUB_WAREHOUSE_PREFIX.length);
              const warehouse = orchestrator.state.githubSetup?.warehouses.find(
                (w) => w.name === name
              );
              if (warehouse) void actions.saveGithubWarehouseCreds(warehouse, values);
            } else {
              void actions.testAndSaveWarehouse(values);
            }
          }}
          onFileUpload={actions.uploadWarehouseFiles}
        />
      );

    case "table_selector":
      return (
        <TableSelector
          schemas={orchestrator.state.discoveredSchemas}
          onExpandSchema={actions.fetchSchemaTables}
          onConfirm={(tables) => orchestrator.setSelectedTables(tables)}
        />
      );

    case "confirm_button":
      return (
        <Button
          size='sm'
          className='self-start'
          onClick={() => {
            if (message.id === "schema_discovery") {
              // Reset error and retry — goToStep re-triggers the discovery effect
              orchestrator.goToStep("schema_discovery");
            } else if (message.id === "building") {
              // Retry build goes through the parent handler so any in-flight
              // phase runs are cancelled first; otherwise zombie SSEs could
              // land late terminal events and dirty the new run.
              if (onRetryBuild) {
                onRetryBuild();
              } else {
                orchestrator.goToStep("building");
              }
            }
          }}
        >
          {block.label}
        </Button>
      );

    case "none":
      return null;

    default:
      return null;
  }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/** Check if a suspension question is a file_change that should be auto-accepted */
function isAutoAcceptableQuestion(
  questions: Array<{ prompt: string; suggestions?: string[] }>
): boolean {
  return questions.some((q) => {
    try {
      const parsed = JSON.parse(q.prompt);
      return parsed.type === "file_change";
    } catch {
      return q.suggestions?.some(
        (s) => s.toLowerCase() === "accept" || s.toLowerCase() === "reject"
      );
    }
  });
}
