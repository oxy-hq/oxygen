/**
 * Automation run page — consumes the new `/agentic-automations` API.
 *
 * Layout mirrors the legacy `AutomationPreview`:
 *   - Header (file path) on top.
 *   - Diagram fills the rest; run/stop/retry buttons float at the top right
 *     and a logs toggle floats at the bottom right.
 *   - When the user opens the logs panel, a resizable right sidebar
 *     (`RunSidebar`) hosts the run-history dropdown, "Show logs" toggle,
 *     step list, and event log.
 *
 * The sidebar closes via its own X button or the floating logs button.
 */

import { ReactFlowProvider } from "@xyflow/react";
import { LogsIcon } from "lucide-react";
import type React from "react";
import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";

import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import { Button } from "@/components/ui/shadcn/button";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import useSidebar from "@/components/ui/shadcn/sidebar-context";
import {
  useAgenticAutomationConfig,
  useAutomationRunController,
  useAutomationRunSnapshot
} from "@/hooks/api/agentic-automations/useAgenticAutomations";
import { decodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import type { AutomationConfig } from "@/stores/useAutomation";

import { extractIterationsBySteps } from "./components/IterationGrid";
import { type RetryOptions, RunControls } from "./components/RunControls";
import { RunSidebar } from "./components/RunSidebar";
import { RunStatusProvider } from "./components/RunStatusContext";
import AutomationPageHeader from "./Header";

// React-Flow + dagre pull in ~150KB; keep the diagram async so the
// initial run-page render doesn't pay for it on slow networks.
const AutomationDiagram = lazy(() => import("@/components/automation/AutomationDiagram"));

export const Automation: React.FC<{
  pathb64: string;
  runId?: string;
  /**
   * Panel orientation for the diagram vs. output split.
   * - `horizontal` (default): diagram on the left, output on the right.
   *   Used by the full-page `AutomationPage` route.
   * - `vertical`: diagram on top, output below. Used when embedded in
   *   a narrower column — the IDE automation editor's preview pane and
   *   the thread file-preview panel both pass this.
   */
  direction?: "horizontal" | "vertical";
  /**
   * Skip the file-path header at the top. Embedded consumers (the IDE
   * editor wraps the preview in `EditorPageWrapper` which already
   * draws a header; the thread file-preview pane has its own chrome)
   * pass `true`.
   */
  hideHeader?: boolean;
}> = ({ pathb64, runId: initialRunId, direction = "horizontal", hideHeader = false }) => {
  const path = useMemo(() => decodeBase64(pathb64), [pathb64]);
  const [, setSearchParams] = useSearchParams();
  const [showLogs, setShowLogs] = useState(true);
  // Mirrors the legacy behavior: if the user landed with a runId in the URL,
  // open the sidebar by default; otherwise show only the diagram.
  const [showOutput, setShowOutput] = useState(!!initialRunId);
  // Mobile UX: force vertical stacking (diagram on top, output below)
  // and offer a fullscreen toggle on the output panel so users on
  // phones can read logs without the diagram squeezing them. Desktop
  // keeps whatever `direction` the caller passed.
  const { isMobile } = useSidebar();
  const effectiveDirection = isMobile ? "vertical" : direction;
  const [outputFullScreen, setOutputFullScreen] = useState(false);
  // Loop step name currently focused for the sidebar's live
  // iteration view. Set by `LoopProgressBar`'s click; cleared when
  // the user switches sidebar tabs or closes the sidebar. `null`
  // when no loop is selected.
  const [selectedLoop, setSelectedLoop] = useState<string | null>(null);

  const controller = useAutomationRunController();

  // Adopt the URL-supplied runId on first render and on browser back/forward.
  //
  // IMPORTANT: only depend on `initialRunId`. The URL effect below writes
  // `?run=<id>` *after* the user selects a run, which fires this effect on the
  // next pass. If we also depended on `controller.runId`, the user's selection
  // would race the URL write: the effect would re-fire with the still-stale
  // URL value and revert the selection.
  useEffect(() => {
    if (initialRunId) {
      controller.setRunId(initialRunId);
      setShowOutput(true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialRunId, controller.setRunId]);

  // Keep the URL `?run=` query param in sync with the active run for refresh.
  useEffect(() => {
    if (!controller.runId) return;
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.set("run", controller.runId as string);
        return next;
      },
      { replace: true }
    );
  }, [controller.runId, setSearchParams]);

  const handleRun = useCallback(async () => {
    setShowOutput(true);
    await controller.launch({ workflow_ref: path });
  }, [controller, path]);

  const handleRetry = useCallback(
    async (opts: RetryOptions) => {
      if (!controller.runId) return;
      setShowOutput(true);
      await controller.launch({
        workflow_ref: path,
        retry_from_run_id: controller.runId,
        cache_enabled: opts.cacheEnabled,
        invalidate_iterations: opts.invalidateIterations
      });
    },
    [controller, path]
  );

  // Diagram per-step replay: the node's refresh button calls this. We
  // also force the output sidebar open so the user sees the new run's
  // logs immediately rather than wondering whether anything happened.
  const handleReplayStep = useCallback(
    async (stepName: string) => {
      setShowOutput(true);
      await controller.replayStep(path, stepName);
    },
    [controller, path]
  );

  const phase = controller.stream.phase;
  const running = phase === "starting" || phase === "running";
  const hasPriorRun = !!controller.runId && phase !== "running" && phase !== "starting";

  // Pull the prior run's snapshot so the Retry popover can render its
  // iteration override grid. Only fetched when we have a finished run
  // to retry against — otherwise there's nothing to show.
  const priorSnapshot = useAutomationRunSnapshot(hasPriorRun ? controller.runId : undefined);
  const iterationsBySteps = useMemo(
    () => extractIterationsBySteps(priorSnapshot.data?.results),
    [priorSnapshot.data?.results]
  );

  // The diagram only reads `tasks[].name + .type` for layout, so the
  // permissive `AutomationConfigShape` from the parser is structurally
  // compatible with the legacy `AutomationConfig` type.
  const configQuery = useAgenticAutomationConfig(pathb64);
  const automationConfig = configQuery.data as AutomationConfig | undefined;

  return (
    <div className='flex h-full w-full flex-col'>
      {!hideHeader && <AutomationPageHeader path={path} />}
      <ResizablePanelGroup direction={effectiveDirection} className='flex-1'>
        <ResizablePanel
          defaultSize={showOutput ? 60 : 100}
          minSize={30}
          className={cn(outputFullScreen && "hidden")}
        >
          <div className='relative h-full w-full'>
            {automationConfig ? (
              <RunStatusProvider
                steps={controller.stream.steps}
                onReplayStep={handleReplayStep}
                onSelectLoop={(name) => {
                  setSelectedLoop(name);
                  // Auto-open the sidebar so the click has an
                  // immediate effect even when the user closed it
                  // earlier in this session.
                  setShowOutput(true);
                }}
              >
                <ReactFlowProvider>
                  <Suspense fallback={<LoadingSkeleton className='h-full w-full' />}>
                    <AutomationDiagram
                      automationId={pathb64}
                      runId={controller.runId}
                      automationConfig={automationConfig}
                    />
                  </Suspense>
                </ReactFlowProvider>
              </RunStatusProvider>
            ) : (
              <LoadingSkeleton className='h-full w-full' />
            )}

            <div className='absolute top-4 right-4 flex items-center gap-2'>
              <RunControls
                running={running}
                hasPriorRun={hasPriorRun}
                iterationsBySteps={iterationsBySteps}
                starting={controller.starting}
                cancelling={controller.cancelling}
                onRun={handleRun}
                onRetry={handleRetry}
                onStop={controller.stop}
              />
            </div>

            {!showOutput && (
              <div className='absolute right-4 bottom-4 flex items-center gap-2'>
                <Button
                  variant='outline'
                  size='icon'
                  onClick={() => setShowOutput(true)}
                  tooltip='Show Logs Output'
                  aria-label='Show logs output'
                >
                  <LogsIcon className='size-4' />
                </Button>
              </div>
            )}
          </div>
        </ResizablePanel>

        {showOutput && (
          <>
            {!outputFullScreen && <ResizableHandle withHandle />}
            <ResizablePanel
              defaultSize={40}
              minSize={20}
              className={cn(outputFullScreen && "flex-1!")}
            >
              <RunSidebar
                automationRef={path}
                runId={controller.runId ?? undefined}
                onSelectRun={(id) => controller.setRunId(id)}
                events={controller.stream.events}
                phase={phase}
                error={controller.stream.error}
                showLogs={showLogs}
                onShowLogsChange={setShowLogs}
                onClose={() => setShowOutput(false)}
                steps={controller.stream.steps}
                selectedLoop={selectedLoop}
                onSelectedLoopChange={setSelectedLoop}
                isFullScreen={outputFullScreen}
                onToggleFullScreen={() => setOutputFullScreen((prev) => !prev)}
              />
            </ResizablePanel>
          </>
        )}
      </ResizablePanelGroup>
    </div>
  );
};

const AutomationPage = () => {
  const { pathb64 } = useParams();
  const [searchParams] = useSearchParams();
  const runId = searchParams.get("run") || undefined;
  return <Automation key={pathb64} pathb64={pathb64 ?? ""} runId={runId} />;
};

export default AutomationPage;
