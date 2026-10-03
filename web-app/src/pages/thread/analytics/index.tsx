import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactNode, RefObject } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import BuilderMessageInput from "@/components/BuilderMessageInput";
import ThinkingModeMenu from "@/components/Chat/ChatPanel/ThinkingModeMenu";
import Markdown from "@/components/Markdown";
import UserMessage from "@/components/Messages/UserMessage";
import ErrorAlert from "@/components/ui/ErrorAlert";
import IdeUnavailablePanel from "@/components/ui/IdeUnavailablePanel";
import { Button } from "@/components/ui/shadcn/button";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import useSidebar from "@/components/ui/shadcn/sidebar-context";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type {
  ArtifactItem,
  AutomationItem,
  BuilderDelegationItem,
  SelectableItem,
  SqlItem
} from "@/hooks/analyticsSteps";
import queryKeys from "@/hooks/api/queryKey";
import useBuilderAvailable from "@/hooks/api/useBuilderAvailable";
import type { AnalyticsDisplayBlock, SseEvent } from "@/hooks/useAnalyticsRun";
import {
  extractAnswer,
  extractDisplayBlocks,
  sseEventToUiBlock,
  uiBlockToSseEvent,
  useAnalyticsRun
} from "@/hooks/useAnalyticsRun";
import type { BuilderFileChange } from "@/hooks/useBuilderActivity";
import {
  extractChangeDecision,
  extractFileChangedMetadata,
  useBuilderActivity
} from "@/hooks/useBuilderActivity";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import type {
  AnalyticsRunSummary,
  FileChangedBlock,
  FileChangePendingBlock,
  ThinkingMode,
  UiBlock
} from "@/services/api/analytics";
import { AnalyticsService } from "@/services/api/analytics";
import { consumePendingThinkingMode } from "@/stores/analyticsThinkingMode";
import type { ThreadItem } from "@/types/chat";
import AcceptedChangePills from "./AcceptedChangePills";
import AnalyticsArtifactSidebar from "./AnalyticsArtifactSidebar";
import AnalyticsReasoningTrace from "./AnalyticsReasoningTrace";
import { AnalyticsDisplayBlockItem } from "./analyticsArtifactHelpers";
import BuilderActivityPanel from "./BuilderActivityPanel";
import BuilderDelegationPanel from "./BuilderDelegationPanel";
import { parseFileChange } from "./FileChangeDiff";
import FilePreviewPanel from "./FilePreviewPanel";
import Header from "./Header";
import MessageInputShell from "./MessageInputShell";
import SuspensionPrompt from "./SuspensionPrompt";

/** Answer text the backend interprets as an approval for proposed changes. */
const ACCEPT_ANSWER = "Accept";

/**
 * A trace artifact open in the sidebar and the run it came from. Trace item ids restart
 * at 0 in every run, so the run id is what tells two runs' pills apart.
 */
type ArtifactSelection = { runId: string; item: ArtifactItem | SqlItem | AutomationItem };

/** For a run entry with no events yet: its trace has no pills to pick. */
const NO_PILLS = () => undefined;

interface Props {
  thread: ThreadItem;
  /** Hide the page header when embedded in the Ask dock. */
  hideHeader?: boolean;
}

// ── Scroll-to-bottom behavior ─────────────────────────────────────────────────

function useScrollToBottom(
  containerRef: RefObject<HTMLDivElement | null>,
  bottomRef: RefObject<HTMLDivElement | null>
) {
  const isUserScrolledUp = useRef(false);

  // biome-ignore lint/correctness/useExhaustiveDependencies: containerRef is a stable ref object — .current cannot be tracked by React
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const onScroll = () => {
      isUserScrolledUp.current =
        container.scrollHeight - container.scrollTop - container.clientHeight > 100;
    };
    container.addEventListener("scroll", onScroll);
    return () => container.removeEventListener("scroll", onScroll);
  }, []);

  // Runs after every render; the scroll guard makes it cheap.
  useEffect(() => {
    if (!isUserScrolledUp.current) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  });

  /** Reset scroll tracking so the next render auto-scrolls to bottom. */
  const scrollToBottom = useCallback(() => {
    isUserScrolledUp.current = false;
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [bottomRef]);

  return { scrollToBottom };
}

// ── Shared run layout ──────────────────────────────────────────────────────────

interface RunEntryProps {
  question: string;
  events: UiBlock[];
  isRunning: boolean;
  isBuilder?: boolean;
  onSelectArtifact: (item: SelectableItem) => void;
  /** Whether this run's trace item is the one on screen — its pill's pressed state. */
  isSelected?: (item: SelectableItem) => boolean;
  acceptedChanges?: BuilderFileChange[];
  onSelectChange?: (change: BuilderFileChange) => void;
  selectedChangeId?: string;
  children?: ReactNode;
}

const RunEntry = ({
  question,
  events,
  isRunning,
  isBuilder,
  onSelectArtifact,
  isSelected,
  acceptedChanges,
  onSelectChange,
  selectedChangeId,
  children
}: RunEntryProps) => (
  <div className='mb-8'>
    <div className='mb-4 flex justify-end'>
      <UserMessage content={question} />
    </div>
    {(events.length > 0 || isRunning) && (
      <div className='mb-4'>
        <AnalyticsReasoningTrace
          events={events}
          isRunning={isRunning}
          onSelectArtifact={onSelectArtifact}
          isSelected={isSelected}
        />
      </div>
    )}
    {children}
    {isBuilder && acceptedChanges && acceptedChanges.length > 0 && onSelectChange && (
      <AcceptedChangePills
        changes={acceptedChanges}
        onSelect={onSelectChange}
        selectedId={selectedChangeId}
      />
    )}
  </div>
);

// ── Completed run (rendered from REST data) ───────────────────────────────────

const PastRunEntry = ({
  run,
  onSelectArtifact,
  isSelected,
  onSelectChange,
  selectedChangeId,
  capturedChanges
}: {
  run: AnalyticsRunSummary;
  onSelectArtifact: (item: SelectableItem, runId: string, blocks: AnalyticsDisplayBlock[]) => void;
  isSelected: (runId: string, item: SelectableItem) => boolean;
  onSelectChange?: (change: BuilderFileChange) => void;
  selectedChangeId?: string;
  capturedChanges?: BuilderFileChange[];
}) => {
  const isBuilder = run.agent_id === "__builder__";
  const runSseEvents = useMemo(() => (run.ui_events ?? []).map(uiBlockToSseEvent), [run.ui_events]);
  const runBlocks = useMemo(() => extractDisplayBlocks(runSseEvents), [runSseEvents]);
  const runAnswer = useMemo(
    () => run.answer ?? extractAnswer(runSseEvents),
    [run.answer, runSseEvents]
  );
  // Use changes captured at run-end when available (current session); fall back
  // to server ui_events so pills survive a page reload.
  const acceptedChanges = useMemo((): BuilderFileChange[] => {
    if (capturedChanges) return capturedChanges;
    if (!isBuilder || run.status !== "done") return [];
    const events = run.ui_events ?? [];
    let counter = 0;
    // Use file_changed events (emitted only on actual writes) for accurate past-run pills.
    const fileChangedEvents = events.filter(
      (ev): ev is FileChangedBlock => ev.event_type === "file_changed"
    );
    if (fileChangedEvents.length > 0) {
      return fileChangedEvents.map((ev) => ({
        kind: "file_changed" as const,
        id: `past-${run.run_id}-change-${counter++}`,
        filePath: ev.payload.file_path,
        description: ev.payload.description,
        newContent: ev.payload.new_content,
        oldContent: ev.payload.old_content,
        isDeletion: ev.payload.is_deletion,
        status: "accepted" as const
      }));
    }
    // Legacy fallback: older runs without file_changed events — treat all file_change_pending
    // events as accepted (same as before).
    return events
      .filter((ev): ev is FileChangePendingBlock => ev.event_type === "file_change_pending")
      .reduce<BuilderFileChange[]>((acc, ev) => {
        const decision = extractChangeDecision(events, ev.seq);
        if (decision !== "accepted") return acc;
        const { oldContent, isDeletion } = extractFileChangedMetadata(events, ev.seq);
        acc.push({
          kind: "file_changed" as const,
          id: `past-${run.run_id}-change-${counter++}`,
          filePath: ev.payload.file_path,
          description: ev.payload.description,
          newContent: ev.payload.new_content,
          oldContent,
          isDeletion: ev.payload.delete ?? isDeletion,
          status: "accepted" as const
        });
        return acc;
      }, []);
  }, [capturedChanges, isBuilder, run.run_id, run.status, run.ui_events]);

  return (
    <RunEntry
      question={run.question}
      events={run.ui_events ?? []}
      isRunning={false}
      isBuilder={isBuilder}
      onSelectArtifact={(item) => onSelectArtifact(item, run.run_id, runBlocks)}
      isSelected={(item) => isSelected(run.run_id, item)}
      acceptedChanges={acceptedChanges}
      onSelectChange={onSelectChange}
      selectedChangeId={selectedChangeId}
    >
      {run.status === "done" && (
        <div className='flex flex-col gap-4'>
          {run.ui_events &&
            extractDisplayBlocks(run.ui_events.map((e) => uiBlockToSseEvent(e))).map((block, i) => {
              const key = `${block.config.chart_type}-${block.config.title ?? i}`;
              return (
                <AnalyticsDisplayBlockItem key={key} block={block} index={i} runId={run.run_id} />
              );
            })}
          {runAnswer && (
            <div>
              <Markdown>{runAnswer}</Markdown>
            </div>
          )}
        </div>
      )}
      {run.status === "failed" && (
        <ErrorAlert title='Run failed'>
          {run.error_message && <Markdown>{run.error_message}</Markdown>}
        </ErrorAlert>
      )}
      {run.status === "cancelled" && (
        <div className='rounded-lg border border-border bg-muted p-4 text-center'>
          <p className='text-muted-foreground text-sm'>Operation cancelled</p>
        </div>
      )}
    </RunEntry>
  );
};

// ── Thread ────────────────────────────────────────────────────────────────────

const AnalyticsThread = ({ thread, hideHeader }: Props) => {
  const { project, branchName } = useCurrentProjectBranch();
  const { isMobile } = useSidebar();
  const bottomRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [followUpQuestion, setFollowUpQuestion] = useState("");
  const [selectedArtifact, setSelectedArtifact] = useState<ArtifactSelection | null>(null);
  const [activeQuestion, setActiveQuestion] = useState<string | null>(null);
  const [builderPanelOpen, setBuilderPanelOpen] = useState(false);
  const [changeDecisions, setChangeDecisions] = useState<Map<number, "accepted" | "rejected">>(
    () => new Map()
  );
  const [selectedFileChange, setSelectedFileChange] = useState<BuilderFileChange | null>(null);
  const [selectedDelegation, setSelectedDelegation] = useState<BuilderDelegationItem | null>(null);
  const [selectedDisplayBlocks, setSelectedDisplayBlocks] = useState<AnalyticsDisplayBlock[]>([]);
  // Accepted changes captured per-run when a run reaches terminal state, so they
  // survive the live→PastRunEntry transition (streamingEvents clears on reset).
  const [capturedRunChanges, setCapturedRunChanges] = useState<Map<string, BuilderFileChange[]>>(
    () => new Map()
  );
  const [autoApprove, setAutoApprove] = useState(
    () => localStorage.getItem("builder_auto_approve") === "true"
  );
  const [thinkingMode, setThinkingMode] = useState<ThinkingMode>(
    () => consumePendingThinkingMode(thread.id) ?? "auto"
  );

  const handleAutoApproveChange = useCallback((value: boolean) => {
    setAutoApprove(value);
    localStorage.setItem("builder_auto_approve", String(value));
  }, []);

  const hasSyncedThinkingMode = useRef(false);

  const { scrollToBottom } = useScrollToBottom(containerRef, bottomRef);

  const queryClient = useQueryClient();
  const { state, start, reconnect, answer, stop, reset, isStarting, isAnswering } = useAnalyticsRun(
    { projectId: project.id }
  );
  // Keep a stable ref so effects that only run on isTerminal can read the
  // current events without listing state as a reactive dependency.
  const stateRef = useRef(state);
  stateRef.current = state;
  // Track latest accepted changes so the isTerminal effect can capture them
  // before streamingEvents is cleared by reset().
  const liveAcceptedChangesRef = useRef<BuilderFileChange[]>([]);
  // Track which terminal runs have already auto-opened the file preview so we
  // don't re-trigger when the user navigates away from the preview manually.
  const autoOpenedRunIdRef = useRef<string | null>(null);

  const {
    data: allRuns = [],
    isLoading: isLookingUp,
    isFetching: isFetchingRuns
  } = useQuery({
    queryKey: queryKeys.analytics.runsByThread(project.id, thread.id),
    queryFn: () => AnalyticsService.getRunsByThread(project.id, thread.id)
  });

  const latestRun = allRuns.at(-1) ?? null;

  const handleThinkingModeChange = useCallback(
    (mode: ThinkingMode) => {
      setThinkingMode(mode);
      const runId = latestRun?.run_id;
      if (runId) {
        AnalyticsService.updateThinkingMode(project.id, runId, mode).catch(() => {});
      }
    },
    [latestRun, project.id]
  );

  // Page load: reconnect SSE only for active runs. Terminal runs render via allRuns.
  useEffect(() => {
    if (state.tag !== "idle" || !latestRun) return;
    if (latestRun.status === "running" || latestRun.status === "suspended") {
      reconnect(latestRun.run_id, latestRun.status);
    }
  }, [latestRun, state.tag, reconnect]);

  // When a run reaches a terminal state, invalidate allRuns so the completed run
  // appears with its ui_events on the next render.
  const isTerminal = state.tag === "done" || state.tag === "failed" || state.tag === "cancelled";
  useEffect(() => {
    if (!isTerminal) return;
    const s = stateRef.current;
    // Capture accepted changes before streamingEvents clears on reset(), so
    // PastRunEntry can still show pills after the live→history transition.
    if ("runId" in s && s.runId) {
      const accepted = liveAcceptedChangesRef.current;
      if (accepted.length > 0) {
        setCapturedRunChanges((prev) => new Map(prev).set(s.runId, accepted));
      }
    }
    queryClient.invalidateQueries({
      queryKey: queryKeys.analytics.runsByThread(project.id, thread.id)
    });
    // When the builder has accepted changes, selectively invalidate queries
    // based on which file types were actually modified.
    if (liveAcceptedChangesRef.current.length > 0) {
      const paths = liveAcceptedChangesRef.current.map((c) => c.filePath);
      const hasAutomation = paths.some(
        (p) => p.endsWith(".automation.yml") || p.endsWith(".procedure.yml")
      );
      const hasApp = paths.some((p) => p.endsWith(".app.yml"));

      // Files are always invalidated — any accepted change writes to disk.
      queryClient.invalidateQueries({ queryKey: queryKeys.file.all(project.id, branchName) });
      // Header CTA gates on revisionInfo.uncommitted_count, so refresh
      // it whenever the agent pipeline has written files to the working tree.
      queryClient.invalidateQueries({
        queryKey: queryKeys.workspaces.revisionInfo(project.id, branchName)
      });
      if (hasAutomation) {
        queryClient.invalidateQueries({
          queryKey: queryKeys.automation.list(project.id, branchName)
        });
      }
      if (hasApp) {
        // Eagerly refetch display structure so the preview reflects layout changes
        // immediately. Data queries are intentionally left alone — they're expensive
        // and should only re-run on explicit refresh or window focus.
        for (const sub of ["list", "getDisplays"] as const) {
          queryClient.refetchQueries({
            queryKey: [...queryKeys.app.all, sub, project.id, branchName],
            type: "all"
          });
        }
      }
    }
  }, [isTerminal, queryClient, project.id, branchName, thread.id]);

  // Once allRuns reflects the terminal run, reset state to idle so it transitions
  // to a PastRunEntry. Uses the runId string (stable) rather than the full state object.
  const terminalRunId = isTerminal && "runId" in state ? state.runId : null;
  useEffect(() => {
    if (!terminalRunId) return;
    const reflected = allRuns.some(
      (r) => r.run_id === terminalRunId && (r.status === "done" || r.status === "failed")
    );
    if (reflected) reset();
  }, [terminalRunId, allRuns, reset]);

  // Clear the tracked question once the run is idle (terminal → PastRunEntry transition done).
  useEffect(() => {
    if (state.tag === "idle") setActiveQuestion(null);
  }, [state.tag]);

  // Restore the thinking mode from the most recent run once the run list loads.
  // Only syncs once per thread so the user's in-session selection isn't overridden.
  // Once per mount is once per thread: the thread page keys this component by thread
  // id, so another thread is a fresh mount and nothing here resets on a thread change.
  useEffect(() => {
    if (hasSyncedThinkingMode.current || isLookingUp || isFetchingRuns) return;
    hasSyncedThinkingMode.current = true;
    if (latestRun?.thinking_mode) {
      setThinkingMode(latestRun.thinking_mode);
    }
  }, [latestRun, isLookingUp, isFetchingRuns]);

  // ── Derived state ──────────────────────────────────────────────────────────

  const agentId = thread.source;
  const question = thread.input;
  const isBuilder = agentId === "__builder__";

  const {
    builderModel,
    isLoading: isCheckingBuilder,
    isError: builderCheckFailed
  } = useBuilderAvailable();

  const isStreaming = state.tag === "running" || state.tag === "suspended";
  const runExists = isStreaming || isTerminal;

  // Auto-open the builder panel only when a file change is proposed (agent suspended).
  // Suspensions for manage_directory, ask_user, etc. are handled inline — don't open
  // the panel for those. Also skip when auto-approve is on.
  useEffect(() => {
    if (!isBuilder || state.tag !== "suspended") return;
    if (autoApprove) return;
    const isFileChange =
      state.questions.length === 1 && !!parseFileChange(state.questions[0].prompt);
    if (!isFileChange) return;
    setBuilderPanelOpen(true);
    setSelectedFileChange(null);
  }, [isBuilder, state, autoApprove]);
  // Exclude the active run from history while it is being streamed / transitioning to
  // PastRunEntry to avoid rendering it twice (once live, once via allRuns).
  const activeRunId = state.tag !== "idle" && "runId" in state && state.runId ? state.runId : null;
  const historyRuns = activeRunId ? allRuns.filter((r) => r.run_id !== activeRunId) : allRuns;

  const streamingEvents = runExists ? state.events.map(sseEventToUiBlock) : ([] as UiBlock[]);

  // Guard against stale-cache duplicates: when React Query returns a cached []
  // while a background refetch is in progress (isFetchingRuns=true), we must wait
  // for the refetch to complete before concluding this is truly a first visit.
  // Without this, navigating back to a thread whose run hasn't finished yet would
  // see allRuns=[] + isLoading=false and fire a second auto-start run.
  const hasNoRunYet =
    !isLookingUp && !isFetchingRuns && allRuns.length === 0 && state.tag === "idle";
  const isFirstVisit =
    hasNoRunYet &&
    // For builder threads, wait until the model is resolved (covers both in-flight and error cases).
    !(isBuilder && (isCheckingBuilder || !builderModel));
  // The builder check has settled without a model — the request failed, the workspace
  // has no `builder_agent`, or its `model` is empty. Auto-start is the only thing that
  // starts a builder thread's first run and it never fires here, so the thread says why
  // instead of sitting blank.
  const builderCannotStart = hasNoRunYet && isBuilder && !isCheckingBuilder && !builderModel;

  // Auto-start the run on first visit so the user doesn't need to click a button
  // after already submitting their question from ChatPanel.
  useEffect(() => {
    if (isFirstVisit) {
      start(agentId, question, thread.id, thinkingMode, builderModel);
    }
  }, [isFirstVisit, agentId, question, thread.id, start, thinkingMode, builderModel]);

  // For new starts / follow-ups use the locally-tracked question so the UI is responsive
  // before allRuns has picked up the new run. Fall back to latestRun for reconnects.
  const currentQuestion = (runExists ? activeQuestion : null) ?? latestRun?.question ?? question;

  // Builder activity derived from the live event stream.
  const builderActivityItems = useBuilderActivity(streamingEvents, changeDecisions);

  const handleStart = (q: string) => {
    setActiveQuestion(q);
    setChangeDecisions(new Map());
    scrollToBottom();
    start(agentId, q, thread.id, thinkingMode, builderModel);
  };

  const handleSend = () => {
    const q = followUpQuestion.trim();
    if (!q) return;
    setFollowUpQuestion("");
    handleStart(q);
  };

  // Wrap answer to record accept/reject decisions for the builder activity panel.
  const handleAnswer = useCallback(
    (text: string) => {
      if (isBuilder && state.tag === "suspended") {
        // Mark only file_change_pending events that haven't been decided yet —
        // sequential suspension prompts one file at a time so earlier decisions
        // must not be overwritten by the answer to a later file.
        const pendingSeqs = streamingEvents
          .filter((ev) => ev.event_type === "file_change_pending")
          .map((ev) => ev.seq);
        if (pendingSeqs.length > 0) {
          const decision = text.toLowerCase().includes("accept") ? "accepted" : "rejected";
          setChangeDecisions((prev) => {
            const next = new Map(prev);
            for (const seq of pendingSeqs) {
              if (!prev.has(seq)) next.set(seq, decision);
            }
            return next;
          });
          if (decision === "accepted") {
            setBuilderPanelOpen(false);
          }
        }
      }
      answer(text);
    },
    [isBuilder, state.tag, streamingEvents, answer]
  );

  // Auto-approve proposed changes and directory operations when the toggle is enabled.
  useEffect(() => {
    if (!autoApprove || state.tag !== "suspended") return;
    if (state.questions.length !== 1) return;
    const prompt = state.questions[0].prompt;
    const isAutoApprovable = (() => {
      if (parseFileChange(prompt)) return true;
      try {
        const parsed = JSON.parse(prompt);
        return parsed?.type === "manage_directory";
      } catch {
        return false;
      }
    })();
    if (isAutoApprovable) handleAnswer(ACCEPT_ANSWER);
  }, [autoApprove, state, handleAnswer]);

  const handleSelectFileChange = useCallback((change: BuilderFileChange) => {
    setSelectedFileChange((prev) => (prev?.id === change.id ? null : change));
    setSelectedArtifact(null);
    setBuilderPanelOpen(false);
  }, []);

  const liveAcceptedChanges = useMemo(
    () =>
      builderActivityItems.filter(
        (i): i is BuilderFileChange => i.kind === "file_changed" && i.status === "accepted"
      ),
    [builderActivityItems]
  );
  // Keep ref in sync so the isTerminal effect can read current value without a dep.
  liveAcceptedChangesRef.current = liveAcceptedChanges;

  // Auto-open the file preview for the last accepted change once the run
  // terminates. Re-evaluates as `liveAcceptedChanges` grows so we catch
  // accepted file_changed events that arrive a tick after the run-done event.
  // Guarded by run id so the preview isn't re-forced open after the user
  // manually closes it.
  const currentRunId = "runId" in state ? state.runId : null;
  useEffect(() => {
    if (!isTerminal) return;
    if (!currentRunId || autoOpenedRunIdRef.current === currentRunId) return;
    const lastChange = [...liveAcceptedChanges].reverse().find((c) => !c.isDeletion);
    if (!lastChange) return;
    autoOpenedRunIdRef.current = currentRunId;
    setSelectedFileChange(lastChange);
    setSelectedArtifact(null);
    setSelectedDelegation(null);
    setBuilderPanelOpen(false);
  }, [isTerminal, liveAcceptedChanges, currentRunId]);

  // When the builder writes a newer version of the file currently open in the preview
  // panel, update selectedFileChange so FilePreviewPanel remounts with fresh content.
  useEffect(() => {
    if (!selectedFileChange) return;
    const latestForFile = [...liveAcceptedChanges]
      .reverse()
      .find((c) => c.filePath === selectedFileChange.filePath);
    if (latestForFile && latestForFile.id !== selectedFileChange.id) {
      setSelectedFileChange(latestForFile);
    }
  }, [liveAcceptedChanges, selectedFileChange]);

  // The events of the run the open artifact came from: the live stream while that run is
  // the live one, else the run list's copy. Another run's events would show that run's
  // chart (event seqs restart in every run) and its automation step statuses.
  const selectedRunEvents = useMemo((): SseEvent[] => {
    if (!selectedArtifact) return [];
    if ("events" in state && state.runId === selectedArtifact.runId) return state.events;
    const run = allRuns.find((r) => r.run_id === selectedArtifact.runId);
    return run?.ui_events?.map(uiBlockToSseEvent) ?? [];
  }, [selectedArtifact, state, allRuns]);

  // Selecting a delegation or a file change clears the artifact, so the builder panel is
  // the only one that can cover it.
  const artifactShown = selectedArtifact !== null && !(isBuilder && builderPanelOpen);

  // Whether a trace item's panel is the one on screen: the pill's pressed state, and what
  // a click on it reverses. A delegation's panel is never covered; an artifact the builder
  // panel covers reads as not on screen, so its pill brings it back rather than closing it.
  const isOnScreen = useCallback(
    (runId: string, item: SelectableItem) =>
      item.kind === "builder_delegation"
        ? selectedDelegation?.childRunId === item.childRunId
        : artifactShown &&
          selectedArtifact?.runId === runId &&
          selectedArtifact.item.id === item.id,
    [selectedDelegation, artifactShown, selectedArtifact]
  );

  // Re-clicking the pill of what is on screen closes it, as a file-change pill does.
  const handleSelectArtifact = useCallback(
    (item: SelectableItem, runId: string, blocks: AnalyticsDisplayBlock[] = []) => {
      const isOpen = isOnScreen(runId, item);
      if (item.kind === "builder_delegation") {
        setSelectedDelegation(isOpen ? null : item);
        setSelectedArtifact(null);
        setSelectedFileChange(null);
        setBuilderPanelOpen(false);
        return;
      }
      setSelectedDelegation(null);
      setSelectedArtifact(isOpen ? null : { runId, item });
      setSelectedDisplayBlocks(blocks);
      setSelectedFileChange(null);
      setBuilderPanelOpen(false);
    },
    [isOnScreen]
  );

  return (
    <div className='flex h-full flex-col'>
      {!hideHeader && <Header thread={thread} />}

      <ResizablePanelGroup direction={isMobile ? "vertical" : "horizontal"} className='flex-1'>
        <ResizablePanel
          defaultSize={
            isBuilder && builderPanelOpen
              ? 50
              : selectedFileChange
                ? 50
                : selectedArtifact
                  ? 50
                  : 100
          }
          minSize={30}
        >
          <div className='flex h-full w-full flex-1 flex-col py-4'>
            <div
              ref={containerRef}
              className='flex w-full flex-1 flex-col overflow-y-auto [scrollbar-gutter:stable_both-edges]'
            >
              <div className='mx-auto mb-6 w-full max-w-page-content px-4'>
                {(isLookingUp || (isFetchingRuns && allRuns.length === 0)) && (
                  <div className='flex items-center gap-2 text-muted-foreground text-sm'>
                    <Spinner className='size-3' />
                  </div>
                )}

                {historyRuns.map((run) => (
                  <PastRunEntry
                    key={run.run_id}
                    run={run}
                    onSelectArtifact={handleSelectArtifact}
                    isSelected={isOnScreen}
                    onSelectChange={handleSelectFileChange}
                    selectedChangeId={selectedFileChange?.id}
                    capturedChanges={capturedRunChanges.get(run.run_id)}
                  />
                ))}

                {isFirstVisit && (
                  <RunEntry
                    question={question}
                    events={[]}
                    isRunning={true}
                    onSelectArtifact={NO_PILLS}
                  />
                )}

                {builderCannotStart && (
                  <RunEntry
                    question={question}
                    events={[]}
                    isRunning={false}
                    onSelectArtifact={NO_PILLS}
                  >
                    <ErrorAlert
                      title="The Builder Agent didn't start"
                      message={
                        builderCheckFailed
                          ? "Couldn't check this workspace's Builder Agent configuration. Reload the page to try again."
                          : "No model is configured for the Builder Agent. Set builder_agent.model in config.yml, then reload the page."
                      }
                    />
                  </RunEntry>
                )}

                {runExists && (
                  <RunEntry
                    question={currentQuestion}
                    events={streamingEvents}
                    isRunning={isStreaming}
                    isBuilder={isBuilder}
                    onSelectArtifact={(item) =>
                      handleSelectArtifact(
                        item,
                        currentRunId ?? "",
                        state.tag === "done" ? state.displayBlocks : []
                      )
                    }
                    isSelected={(item) => isOnScreen(currentRunId ?? "", item)}
                    acceptedChanges={liveAcceptedChanges}
                    onSelectChange={handleSelectFileChange}
                    selectedChangeId={selectedFileChange?.id}
                  >
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
                          <div>
                            <Markdown>{state.answer}</Markdown>
                          </div>
                        )}
                      </div>
                    )}

                    {state.tag === "failed" &&
                      (state.ideUnavailable ? (
                        <IdeUnavailablePanel
                          description='Your question needs Oxygen Factory, which is restarting. It will run once it is back.'
                          onRetry={() => {
                            reset();
                            handleStart(currentQuestion);
                          }}
                        />
                      ) : (
                        <ErrorAlert
                          title='Run failed'
                          actions={
                            <Button
                              size='sm'
                              variant='outline'
                              onClick={() => {
                                reset();
                                handleStart(currentQuestion);
                              }}
                            >
                              Retry
                            </Button>
                          }
                        >
                          <Markdown>{state.message}</Markdown>
                        </ErrorAlert>
                      ))}
                    {state.tag === "cancelled" && (
                      <div className='rounded-lg border border-border bg-muted p-4 text-center'>
                        <p className='text-muted-foreground text-sm'>Operation cancelled</p>
                      </div>
                    )}
                  </RunEntry>
                )}

                <div ref={bottomRef} />
              </div>
            </div>

            <div className='mx-auto w-full max-w-page-content p-4 pt-0'>
              {state.tag === "suspended" ? (
                <SuspensionPrompt
                  questions={state.questions}
                  onAnswer={handleAnswer}
                  isAnswering={isAnswering}
                />
              ) : isBuilder ? (
                <BuilderMessageInput
                  onSend={handleStart}
                  onStop={stop}
                  disabled={state.tag === "running" || isStarting}
                  isLoading={state.tag === "running" || isStarting}
                  autoApprove={autoApprove}
                  onAutoApproveChange={handleAutoApproveChange}
                />
              ) : (
                <MessageInputShell
                  value={followUpQuestion}
                  onChange={(e: React.ChangeEvent<HTMLTextAreaElement>) =>
                    setFollowUpQuestion(e.target.value)
                  }
                  onKeyDown={(e: React.KeyboardEvent<HTMLTextAreaElement>) => {
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      handleSend();
                    }
                  }}
                  onSend={handleSend}
                  onStop={stop}
                  disabled={state.tag === "running" || isStarting}
                  isLoading={state.tag === "running" || isStarting}
                  extraActions={
                    <ThinkingModeMenu
                      value={thinkingMode}
                      onChange={handleThinkingModeChange}
                      disabled={state.tag === "running" || isStarting}
                    />
                  }
                />
              )}
            </div>
          </div>
        </ResizablePanel>

        {selectedDelegation ? (
          <>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={50} minSize={20} maxSize={70}>
              <BuilderDelegationPanel
                childRunId={selectedDelegation.childRunId}
                projectId={project.id}
                onClose={() => setSelectedDelegation(null)}
              />
            </ResizablePanel>
          </>
        ) : isBuilder && builderPanelOpen ? (
          <>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={50} minSize={20} maxSize={70}>
              <BuilderActivityPanel
                items={builderActivityItems}
                isRunning={isStreaming}
                isSuspended={state.tag === "suspended"}
                onAnswer={handleAnswer}
                isAnswering={isAnswering}
                onClose={() => setBuilderPanelOpen(false)}
              />
            </ResizablePanel>
          </>
        ) : selectedFileChange ? (
          <>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={50} minSize={20} maxSize={70}>
              <FilePreviewPanel
                key={selectedFileChange.id}
                change={selectedFileChange}
                onClose={() => setSelectedFileChange(null)}
              />
            </ResizablePanel>
          </>
        ) : (
          selectedArtifact && (
            <>
              <ResizableHandle withHandle />
              <ResizablePanel defaultSize={50} minSize={20} maxSize={70}>
                <AnalyticsArtifactSidebar
                  item={selectedArtifact.item}
                  displayBlocks={selectedDisplayBlocks}
                  runEvents={selectedRunEvents}
                  isRunning={isStreaming && selectedArtifact.runId === currentRunId}
                  onClose={() => setSelectedArtifact(null)}
                />
              </ResizablePanel>
            </>
          )
        )}
      </ResizablePanelGroup>
    </div>
  );
};

export default AnalyticsThread;
