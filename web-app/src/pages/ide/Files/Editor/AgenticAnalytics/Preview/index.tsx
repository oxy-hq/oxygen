import { cx } from "class-variance-authority";
import { ArrowUp, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import Markdown from "@/components/Markdown";
import EmptyState from "@/components/ui/EmptyState";
import { Button } from "@/components/ui/shadcn/button";
import { Textarea } from "@/components/ui/shadcn/textarea";
import type { SelectableItem } from "@/hooks/analyticsSteps";
import { sseEventToUiBlock, useAnalyticsRun } from "@/hooks/useAnalyticsRun";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useEnterSubmit } from "@/hooks/useEnterSubmit";
import { decodeBase64 } from "@/libs/encoding";
import AnalyticsArtifactSidebar from "@/pages/thread/analytics/AnalyticsArtifactSidebar";
import AnalyticsReasoningTrace from "@/pages/thread/analytics/AnalyticsReasoningTrace";
import { AnalyticsDisplayBlockItem } from "@/pages/thread/analytics/analyticsArtifactHelpers";
import SuspensionPrompt from "@/pages/thread/analytics/SuspensionPrompt";
import type { UiBlock } from "@/services/api/analytics";

/// Build a unique thread ID for the agent-preview surface from the
/// project/branch/path triple. Inlined here after the classic
/// `useAgentThread` store was removed.
const getThreadIdFromPath = (projectId: string, branchName: string, pathb64: string): string =>
  `${projectId}::${branchName}::${pathb64}`;

/**
 * Returns the agent_id for the analytics API from a file path.
 * The backend resolves the config as `project_path.join(agent_id)`, so agent_id
 * must be the path relative to the project root (e.g. "analytics.agentic.yml").
 */
export const getAgentIdFromPath = (filePath: string): string => filePath;

/** Display name shown in the UI (stem only, e.g. "analytics"). */
export const getAgentDisplayName = (filePath: string): string =>
  filePath
    .split("/")
    .at(-1)
    ?.replace(/\.agentic\.(yml|yaml)$/i, "") ?? filePath;

interface AgenticAnalyticsPreviewProps {
  pathb64: string;
}

const AgenticAnalyticsPreview = ({ pathb64 }: AgenticAnalyticsPreviewProps) => {
  const { project, branchName } = useCurrentProjectBranch();
  const threadId = getThreadIdFromPath(project.id, branchName, pathb64);
  const filePath = decodeBase64(pathb64);
  const agentId = getAgentIdFromPath(filePath);
  const displayName = getAgentDisplayName(filePath);

  const { state, start, answer, isStarting, isAnswering } = useAnalyticsRun({
    projectId: project.id
  });

  const [question, setQuestion] = useState("");
  // The artifact a trace row opened, or null.
  //
  // This surface passed `onSelectArtifact={() => {}}`, so every pill and every
  // SQL row in the preview was a styled, hover-responsive button that did
  // nothing — you could not read the semantic query the agent proposed, or the
  // SQL it compiled, from the place you were testing the agent. The thread page
  // has always opened these; only the preview dropped them on the floor.
  const [selectedArtifact, setSelectedArtifact] = useState<SelectableItem | null>(null);
  const { formRef, onKeyDown } = useEnterSubmit();

  // Escape closes the artifact overlay. It covers the whole pane, so without
  // this the only way out is finding the close button — and every other
  // overlay in the product answers Escape, so a reader will try it first.
  useEffect(() => {
    if (!selectedArtifact) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSelectedArtifact(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectedArtifact]);

  const isRunning = state.tag === "running" || state.tag === "suspended";
  const hasStarted = state.tag !== "idle";

  const currentEvents: UiBlock[] = "events" in state ? state.events.map(sseEventToUiBlock) : [];

  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!question.trim() || isRunning || isStarting) return;
    start(agentId, question, threadId);
    setQuestion("");
  };

  return (
    <div className='relative flex h-full flex-col justify-between overflow-hidden'>
      <div className='customScrollbar scrollbar-gutter-auto flex flex-1 flex-col overflow-auto'>
        <div className='flex flex-col gap-4 p-4'>
          {!hasStarted ? (
            <EmptyState
              className='h-full'
              title='No messages yet'
              description={`Ask the ${displayName} agent a question to get started`}
            />
          ) : (
            <>
              {(currentEvents.length > 0 || isRunning) && (
                <AnalyticsReasoningTrace
                  events={currentEvents}
                  isRunning={isRunning}
                  onSelectArtifact={setSelectedArtifact}
                />
              )}

              {state.tag === "done" && (
                <div className='flex flex-col gap-4'>
                  {state.displayBlocks.map((block, i) => {
                    const key = `${block.config.chart_type}-${block.config.title ?? i}`;
                    return (
                      <AnalyticsDisplayBlockItem
                        key={key}
                        block={block}
                        index={i}
                        runId={state.runId}
                      />
                    );
                  })}
                  {state.answer && (
                    <div className='rounded-lg border border-border bg-card p-4'>
                      <Markdown>{state.answer}</Markdown>
                    </div>
                  )}
                </div>
              )}

              {state.tag === "failed" && (
                <div className='rounded-lg border border-destructive bg-destructive/10 p-4'>
                  <p className='font-medium text-destructive text-sm'>Run failed</p>
                  <Markdown>{state.message}</Markdown>
                </div>
              )}

              {state.tag === "cancelled" && (
                <div className='rounded-lg border border-border bg-muted p-4'>
                  <p className='font-medium text-muted-foreground text-sm'>Operation cancelled</p>
                </div>
              )}
            </>
          )}
        </div>
      </div>

      {/* An overlay, not a side-by-side panel: the preview is one column of an
          IDE split that is already narrow, so the thread page's resizable
          second pane would leave neither side readable. Covering the trace
          matches what a reader is doing — inspecting one step — and Escape or
          the close button returns them to it. */}
      {/* `builder_delegation` is in `SelectableItem` but not in the sidebar's
          own prop type — it needs the thread page's builder panel, which this
          surface does not have. Same guard as `OnboardingThread`. Say so rather
          than rendering nothing: a pill that swallows the click is the exact
          bug this overlay was added to fix, and leaving one case behind is how
          it comes back. */}
      {selectedArtifact?.kind === "builder_delegation" && (
        <div
          className='absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 bg-background p-6 text-center'
          data-testid='agent-preview-artifact-unsupported'
        >
          <p className='text-muted-foreground text-sm'>
            Builder steps open in the full thread view — this preview has no builder panel.
          </p>
          <Button size='sm' variant='outline' onClick={() => setSelectedArtifact(null)}>
            Back to the trace
          </Button>
        </div>
      )}
      {selectedArtifact && selectedArtifact.kind !== "builder_delegation" && (
        <div
          className='absolute inset-0 z-20 flex flex-col bg-background'
          data-testid='agent-preview-artifact'
        >
          <AnalyticsArtifactSidebar
            item={selectedArtifact}
            runEvents={"events" in state ? state.events : []}
            isRunning={isRunning}
            onClose={() => setSelectedArtifact(null)}
          />
        </div>
      )}

      <div className='p-4'>
        {state.tag === "suspended" ? (
          <SuspensionPrompt
            questions={state.questions}
            onAnswer={answer}
            isAnswering={isAnswering}
          />
        ) : (
          <form
            ref={formRef}
            onSubmit={handleSubmit}
            className='mx-auto flex w-full max-w-[672px] gap-1 rounded-md border p-2'
          >
            <Textarea
              disabled={isRunning || isStarting}
              name='question'
              autoFocus
              onKeyDown={onKeyDown}
              onChange={(e) => setQuestion(e.target.value)}
              value={question}
              className={cx(
                "bg-transparent",
                "border-none shadow-none",
                "hover:border-none focus-visible:border-none focus-visible:shadow-none",
                "focus-visible:ring-0 focus-visible:ring-offset-0",
                "resize-none outline-none",
                "box-border min-h-[32px]"
              )}
              placeholder={`Ask the ${displayName} agent a question`}
            />
            <Button
              className='h-8 w-8'
              disabled={!question || isRunning || isStarting}
              type='submit'
            >
              {isRunning || isStarting ? <Loader2 className='animate-spin' /> : <ArrowUp />}
            </Button>
          </form>
        )}
      </div>
    </div>
  );
};

export default AgenticAnalyticsPreview;
