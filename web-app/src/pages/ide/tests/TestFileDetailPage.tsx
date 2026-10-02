import {
  AlertCircle,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CirclePlay,
  Clock,
  Copy,
  FileText,
  History,
  Pencil,
  Play,
  Search,
  Square,
  ThumbsDown,
  ThumbsUp,
  Trash2,
  TriangleAlert,
  XCircle,
  Zap
} from "lucide-react";

import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useParams, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { CanWorkspaceAdmin, CanWorkspaceEditor } from "@/components/auth/Can";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { Input } from "@/components/ui/shadcn/input";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useTestFile from "@/hooks/api/tests/useTestFile";
import {
  useCreateTestRun,
  useDeleteTestRun,
  useHumanVerdicts,
  useSetHumanVerdict,
  useTestRunDetail,
  useTestRuns
} from "@/hooks/api/tests/useTestRuns";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import PageHeader from "@/pages/ide/components/PageHeader";
import type { TestRunCaseResult } from "@/services/api/testRuns";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { TestCaseState } from "@/stores/useTestFileResults";
import useTestFileResults, { createCasePrefix } from "@/stores/useTestFileResults";
import {
  EvalEventState,
  type Record as EvalRecord,
  MetricKind,
  type MetricValue
} from "@/types/eval";

// --- Types ---

type HumanVerdict = "pass" | "fail";
type StatusFilter = "all" | "pass" | "fail" | "flaky";
type CaseVerdict = "pass" | "fail" | "flaky" | "running" | "not_run";

interface FileStatsData {
  totalCases: number;
  completedCases: number;
  passingCases: number;
  flakyCases: number;
  avgScore: number | null;
  avgDurationMs: number | null;
  totalTokens: number | null;
}

// --- Helpers ---

const formatDuration = (ms: number) => {
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms)}ms`;
};

const formatTokens = (n: number) => {
  if (n >= 1000000) return `${(n / 1000000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
};

const formatRunLabel = (run: { name: string | null; created_at: string }) => {
  const date = new Date(run.created_at).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit"
  });
  return run.name ? `${run.name} · ${date}` : date;
};

const isErrorRecord = (r: EvalRecord) =>
  r.actual_output?.startsWith("[ERROR]") ||
  (r.duration_ms === 0 && r.input_tokens === 0 && r.cot.startsWith("Run failed"));

const PASS_THRESHOLD = 0.5;

const getConsistency = (metrics: MetricValue[]) => {
  if (metrics.length === 0) return { passing: 0, total: 0 };
  const metric = metrics[0];
  if (metric.type === MetricKind.Recall) {
    const passing = metric.records.filter((r) => r.pass).length;
    return { passing, total: metric.records.length };
  }
  const passing = metric.records.filter((r) => r.score >= PASS_THRESHOLD).length;
  return { passing, total: metric.records.length };
};

const getLiveCaseVerdict = (caseState: TestCaseState): CaseVerdict => {
  if (caseState.state === EvalEventState.Started || caseState.state === EvalEventState.Progress)
    return "running";
  if (!caseState.result) return "not_run";
  const { passing, total } = getConsistency(caseState.result.metrics);
  if (total === 0) return "not_run";
  if (passing === total) return "pass";
  if (passing === 0) return "fail";
  return "flaky";
};

const getHistoricalVerdict = (verdict: string): CaseVerdict => {
  if (verdict === "pass") return "pass";
  if (verdict === "fail") return "fail";
  if (verdict === "flaky") return "flaky";
  return "not_run";
};

/** Apply human override: human "pass"→pass, "fail"→fail, overrides agent verdict */
const applyHumanOverride = (
  agentVerdict: CaseVerdict,
  humanVerdict: HumanVerdict | undefined
): CaseVerdict => {
  if (!humanVerdict || agentVerdict === "running" || agentVerdict === "not_run")
    return agentVerdict;
  return humanVerdict;
};

const scoreColorClass = (pct: number) =>
  pct >= 80
    ? "border-success text-success"
    : pct >= 50
      ? "border-warning text-warning"
      : "border-destructive text-destructive";

/** Verdict sort weight for issues-first ordering */
const verdictWeight = (v: CaseVerdict) => {
  switch (v) {
    case "fail":
      return 0;
    case "flaky":
      return 1;
    case "running":
      return 2;
    case "not_run":
      return 3;
    case "pass":
      return 4;
    default:
      return 5;
  }
};

// --- Sub-components ---

const VerdictIcon: React.FC<{ verdict: CaseVerdict; className?: string }> = ({
  verdict,
  className
}) => {
  const base = cn("shrink-0", className ?? "h-4 w-4");
  switch (verdict) {
    case "pass":
      return <CheckCircle2 className={cn(base, "text-success")} />;
    case "fail":
      return <XCircle className={cn(base, "text-destructive")} />;
    case "flaky":
      return <TriangleAlert className={cn(base, "text-warning")} />;
    case "running":
      return <Spinner className={cn("size-3", base, "text-primary")} />;
    default:
      return <div className={cn(base, "rounded-full border border-muted-foreground/30")} />;
  }
};

const TruncatedText: React.FC<{
  text: string;
  mono?: boolean;
  className?: string;
}> = ({ text, mono, className }) => {
  const [expanded, setExpanded] = useState(false);
  const isLong = text.length > 200 || text.split("\n").length > 4;
  return (
    <div className={cn("space-y-1", className)}>
      <p
        className={cn(
          "whitespace-pre-wrap",
          mono ? "rounded bg-muted p-2 font-mono text-xs" : "text-sm",
          !expanded && isLong && "line-clamp-4"
        )}
      >
        {text}
      </p>
      {isLong && (
        <button
          type='button'
          onClick={() => setExpanded(!expanded)}
          className='text-muted-foreground text-xs hover:text-foreground'
        >
          {expanded ? "Show less ↑" : "Show full ↓"}
        </button>
      )}
    </div>
  );
};

const CopyButton: React.FC<{ text: string }> = ({ text }) => {
  const [copied, setCopied] = useState(false);
  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  }, [text]);
  return (
    <button
      type='button'
      onClick={handleCopy}
      title='Copy to clipboard'
      className='shrink-0 rounded p-1 text-muted-foreground/60 hover:bg-muted hover:text-foreground'
    >
      {copied ? <Check className='h-3 w-3 text-success' /> : <Copy className='h-3 w-3' />}
    </button>
  );
};

/** Section header row with label and optional copy action */
const SectionHeader: React.FC<{
  label: string;
  copyText?: string;
  className?: string;
}> = ({ label, copyText, className }) => (
  <div className={cn("flex items-center justify-between", className)}>
    <p className='font-semibold text-[11px] text-muted-foreground/70 uppercase tracking-wider'>
      {label}
    </p>
    {copyText && <CopyButton text={copyText} />}
  </div>
);

// --- Summary Strip ---

const SummaryStrip: React.FC<{
  stats: FileStatsData | null;
  judgeModel: string | null;
  runs: number;
}> = ({ stats, judgeModel, runs }) => {
  const correctnessPct = stats?.avgScore != null ? Math.round(stats.avgScore * 100) : null;

  return (
    <div className='flex items-center gap-3 border-b bg-muted/20 px-4 py-1.5 text-xs'>
      {/* Primary: correctness pill */}
      {correctnessPct !== null && (
        <Badge variant='outline' className={cn("tabular-nums", scoreColorClass(correctnessPct))}>
          {correctnessPct}%
        </Badge>
      )}
      {/* Primary: flaky badge when present */}
      {stats && stats.flakyCases > 0 && (
        <Badge variant='outline' className='border-warning text-warning tabular-nums'>
          {stats.flakyCases} flaky
        </Badge>
      )}
      {/* Medium: judge + runs */}
      {judgeModel && (
        <span className='text-muted-foreground'>
          <span className='opacity-70'>Judge</span>{" "}
          <span className='font-medium text-foreground/80'>{judgeModel}</span>
        </span>
      )}
      <span className='text-muted-foreground'>
        <span className='opacity-70'>Runs</span>{" "}
        <span className='font-medium text-foreground/80 tabular-nums'>{runs}</span>
      </span>
      {/* Lower: latency + tokens — muted */}
      {stats && stats.completedCases > 0 && (
        <>
          {stats.avgDurationMs !== null && (
            <span className='flex items-center gap-1 text-muted-foreground/60'>
              <Clock className='h-3 w-3' />
              <span className='tabular-nums'>{formatDuration(stats.avgDurationMs)}</span>
            </span>
          )}
          {stats.totalTokens !== null && stats.totalTokens > 0 && (
            <span className='flex items-center gap-1 text-muted-foreground/60'>
              <Zap className='h-3 w-3' />
              <span className='tabular-nums'>{formatTokens(stats.totalTokens)}</span>
            </span>
          )}
        </>
      )}
    </div>
  );
};

// --- Case Detail Panel ---

interface CaseDetailPanelProps {
  index: number;
  /** 0-based position in the filtered case list (for nav counter and prev/next disable) */
  filteredPosition: number;
  totalCases: number;
  testCase: {
    prompt: string;
    expected: string;
    tags: string[];
    tool: string | null;
  };
  caseState: TestCaseState;
  historicalCase: TestRunCaseResult | null;
  isViewingHistorical: boolean;
  selectedAttemptIndex: number;
  onSelectAttempt: (i: number) => void;
  humanOverride: HumanVerdict | null;
  onSetHumanOverride: (v: HumanVerdict | null) => void;
  onRun: () => void;
  onNavigate: (dir: -1 | 1) => void;
}

const CaseDetailPanel: React.FC<CaseDetailPanelProps> = ({
  index,
  filteredPosition,
  totalCases,
  testCase,
  caseState,
  historicalCase,
  isViewingHistorical,
  selectedAttemptIndex,
  onSelectAttempt,
  humanOverride,
  onSetHumanOverride,
  onRun,
  onNavigate
}) => {
  // Check if this case is actively running (from Zustand store, regardless of view mode)
  const isRunning =
    caseState.state === EvalEventState.Started || caseState.state === EvalEventState.Progress;

  const agentVerdict: CaseVerdict =
    isViewingHistorical && !isRunning
      ? historicalCase
        ? getHistoricalVerdict(historicalCase.verdict)
        : "not_run"
      : getLiveCaseVerdict(caseState);
  const verdict = applyHumanOverride(agentVerdict, humanOverride ?? undefined);

  // Attempt records (from live Zustand state — shown during/after active runs)
  const attemptRecords = caseState.result
    ? caseState.result.metrics.flatMap((m) => (m.type !== MetricKind.Recall ? m.records : []))
    : [];

  const activeRecord = attemptRecords.length > 0 ? attemptRecords[selectedAttemptIndex] : null;

  // Scores — prefer live score when available (active/completed run), fall back to historical
  const liveScore =
    caseState.result?.metrics[0]?.score != null
      ? Math.round(caseState.result.metrics[0].score * 100)
      : null;

  const histScore =
    isViewingHistorical && historicalCase ? Math.round(historicalCase.score * 100) : null;

  const displayScore = liveScore ?? histScore;
  const hasPassed = displayScore !== null && displayScore >= 50;

  const humanVerdict = humanOverride;
  const judgeVerdict = displayScore !== null ? (hasPassed ? "pass" : "fail") : null;
  const judgeDisagreement =
    humanVerdict !== null && judgeVerdict !== null && humanVerdict !== judgeVerdict;

  const hasResult = caseState.result !== null || (isViewingHistorical && historicalCase !== null);

  const judgeErrors = isViewingHistorical ? (historicalCase?.errors ?? null) : null;
  const hasJudgeErrors = judgeErrors !== null && judgeErrors.length > 0;

  // Historical judge reasoning
  const hasJudgeReasoning =
    isViewingHistorical &&
    historicalCase?.judge_reasoning &&
    historicalCase.judge_reasoning.length > 0;

  // Historical case metrics
  const histDuration = isViewingHistorical ? historicalCase?.avg_duration_ms : null;
  const histTokens =
    isViewingHistorical && historicalCase
      ? (historicalCase.input_tokens ?? 0) + (historicalCase.output_tokens ?? 0)
      : 0;

  // Keyboard navigation
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Only if not typing in an input
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        onNavigate(-1);
      } else if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        onNavigate(1);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onNavigate]);

  return (
    <div className='flex h-full flex-col'>
      {/* Sticky case header */}
      <div className='sticky top-0 z-10 border-b bg-background/95 px-4 py-3 backdrop-blur supports-[backdrop-filter]:bg-background/60'>
        <div className='flex items-center justify-between gap-2'>
          <div className='min-w-0 flex-1'>
            {/* Row 1: verdict + name + tags */}
            <div className='flex flex-wrap items-center gap-2'>
              <VerdictIcon verdict={verdict} className='h-4 w-4' />
              <span className='font-semibold text-sm'>Case {index + 1}</span>
              {testCase.tags.map((tag) => (
                <span
                  key={tag}
                  className='rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground'
                >
                  {tag}
                </span>
              ))}
            </div>
            {/* Row 2: score + runs + metrics + badges */}
            <div className='mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1'>
              {verdict !== "not_run" && displayScore !== null && (
                <Badge
                  variant='outline'
                  className={cn("text-xs tabular-nums", scoreColorClass(displayScore))}
                >
                  {displayScore}%
                </Badge>
              )}
              {isViewingHistorical && historicalCase && (
                <span className='text-[11px] text-muted-foreground tabular-nums'>
                  {historicalCase.passing_runs}/{historicalCase.total_runs} runs
                </span>
              )}
              {/* Case-level latency/tokens */}
              {isViewingHistorical && histDuration != null && (
                <span className='flex items-center gap-1 text-[11px] text-muted-foreground/60'>
                  <Clock className='h-2.5 w-2.5' />
                  {formatDuration(histDuration)}
                </span>
              )}
              {isViewingHistorical && histTokens > 0 && (
                <span className='flex items-center gap-1 text-[11px] text-muted-foreground/60'>
                  <Zap className='h-2.5 w-2.5' />
                  {formatTokens(histTokens)}
                </span>
              )}
              {isRunning && (
                <Badge variant='outline' className='text-primary text-xs'>
                  Running...
                </Badge>
              )}
              {humanVerdict && (
                <Badge
                  variant='outline'
                  className={cn(
                    "text-xs",
                    humanVerdict === "pass"
                      ? "border-success text-success"
                      : humanVerdict === "fail"
                        ? "border-destructive text-destructive"
                        : ""
                  )}
                >
                  Human: {humanVerdict}
                </Badge>
              )}
              {judgeDisagreement && (
                <span className='flex items-center gap-1 text-warning text-xs'>
                  <TriangleAlert className='h-3 w-3' />
                  Disagrees with judge
                </span>
              )}
            </div>
          </div>
          {/* Navigation + run */}
          <div className='flex shrink-0 items-center gap-0.5'>
            <Button
              variant='ghost'
              size='icon'
              className='h-6 w-6'
              onClick={() => onNavigate(-1)}
              disabled={filteredPosition <= 0}
              title='Previous case (↑)'
            >
              <ChevronLeft className='h-3.5 w-3.5' />
            </Button>
            <span className='min-w-[2.5rem] text-center text-[11px] text-muted-foreground tabular-nums'>
              {filteredPosition + 1}/{totalCases}
            </span>
            <Button
              variant='ghost'
              size='icon'
              className='h-6 w-6'
              onClick={() => onNavigate(1)}
              disabled={filteredPosition >= totalCases - 1}
              title='Next case (↓)'
            >
              <ChevronRight className='h-3.5 w-3.5' />
            </Button>
            <CanWorkspaceEditor>
              <Button
                variant='ghost'
                size='icon'
                className='ml-1 h-7 w-7'
                onClick={onRun}
                disabled={isRunning}
                title='Run this case'
              >
                <CirclePlay className='h-4 w-4' />
              </Button>
            </CanWorkspaceEditor>
          </div>
        </div>
      </div>

      {/* Scrollable content */}
      <div className='min-h-0 flex-1 space-y-5 overflow-y-auto p-4'>
        {/* Progress bar */}
        {isRunning && (
          <div className='flex items-center gap-2'>
            <div className='h-1.5 flex-1 overflow-hidden rounded-full bg-muted'>
              <div
                className='h-full rounded-full bg-primary transition-all'
                style={{
                  width:
                    caseState.progress.total > 0
                      ? `${(caseState.progress.progress / caseState.progress.total) * 100}%`
                      : "0%"
                }}
              />
            </div>
            <span className='text-muted-foreground text-xs'>
              {caseState.progress.progress}/{caseState.progress.total}
            </span>
          </div>
        )}

        {/* SSE error */}
        {caseState.error && <p className='text-destructive text-xs'>{caseState.error}</p>}

        {/* Prompt — always open */}
        <div>
          <SectionHeader label='Prompt' copyText={testCase.prompt} className='mb-1.5' />
          <TruncatedText text={testCase.prompt} />
        </div>

        {/* Attempt chips + selected attempt content (from live/Zustand state) */}
        {attemptRecords.length > 0 && (
          <div className='space-y-3'>
            <div className='space-y-1.5'>
              <p className='font-semibold text-[11px] text-muted-foreground/70 uppercase tracking-wider'>
                Attempts ({attemptRecords.length})
              </p>
              <div className='inline-flex gap-px rounded-md border bg-muted/50 p-0.5'>
                {attemptRecords.map((record, rIdx) => {
                  const crashed = isErrorRecord(record);
                  const passed = !crashed && record.score >= PASS_THRESHOLD;
                  const isActive = selectedAttemptIndex === rIdx;
                  return (
                    <button
                      key={rIdx}
                      type='button'
                      onClick={() => onSelectAttempt(rIdx)}
                      title={crashed ? "Crashed" : `Score: ${Math.round(record.score * 100)}%`}
                      className={cn(
                        "min-w-[28px] rounded-sm px-2 py-0.5 font-medium text-xs tabular-nums transition-colors",
                        isActive
                          ? passed
                            ? "bg-success text-success-foreground shadow-sm"
                            : "bg-destructive text-destructive-foreground shadow-sm"
                          : passed
                            ? "text-success hover:bg-success/10"
                            : "text-destructive hover:bg-destructive/10"
                      )}
                    >
                      {rIdx + 1}
                      {crashed && " \u2717"}
                    </button>
                  );
                })}
              </div>
            </div>

            {activeRecord && (
              <div className='space-y-3 rounded-md border p-3'>
                <div className='flex items-center justify-between text-muted-foreground text-xs'>
                  <span className='font-medium'>Attempt {selectedAttemptIndex + 1}</span>
                  <span className='flex items-center gap-3 text-muted-foreground/60'>
                    {activeRecord.duration_ms > 0 && (
                      <span className='flex items-center gap-1'>
                        <Clock className='h-3 w-3' />
                        {formatDuration(activeRecord.duration_ms)}
                      </span>
                    )}
                    {activeRecord.input_tokens > 0 && (
                      <span className='flex items-center gap-1'>
                        <Zap className='h-3 w-3' />
                        {formatTokens(activeRecord.input_tokens + activeRecord.output_tokens)}
                      </span>
                    )}
                  </span>
                </div>

                {isErrorRecord(activeRecord) ? (
                  <ErrorAlert
                    title='Evaluation failed'
                    message={activeRecord.actual_output ?? activeRecord.cot}
                  />
                ) : (
                  <>
                    {activeRecord.actual_output && (
                      <div>
                        <div className='mb-1 flex items-center justify-between'>
                          <p className='font-semibold text-[11px] text-muted-foreground/70 uppercase tracking-wider'>
                            Actual output
                          </p>
                          <CopyButton text={activeRecord.actual_output} />
                        </div>
                        <TruncatedText text={activeRecord.actual_output} mono />
                      </div>
                    )}
                    {activeRecord.cot && (
                      <Collapsible defaultOpen={activeRecord.score < PASS_THRESHOLD}>
                        <CollapsibleTrigger className='group flex w-full items-center gap-1 font-semibold text-[11px] text-muted-foreground/70 uppercase tracking-wider hover:text-foreground'>
                          <ChevronRight className='h-3 w-3 transition-transform group-data-[state=open]:rotate-90' />
                          Judge reasoning
                        </CollapsibleTrigger>
                        <CollapsibleContent>
                          <p className='mt-1 whitespace-pre-wrap rounded bg-muted p-2 text-xs'>
                            {activeRecord.cot}
                          </p>
                        </CollapsibleContent>
                      </Collapsible>
                    )}
                  </>
                )}
              </div>
            )}
          </div>
        )}

        {/* Historical actual output — only when no live attempt records */}
        {attemptRecords.length === 0 && isViewingHistorical && historicalCase?.actual_output && (
          <div>
            <SectionHeader
              label='Actual output'
              copyText={historicalCase.actual_output}
              className='mb-1.5'
            />
            <TruncatedText text={historicalCase.actual_output} mono />
          </div>
        )}

        {/* Expected — collapsed by default, with inline preview */}
        {testCase.expected && (
          <Collapsible defaultOpen={false}>
            <CollapsibleTrigger className='group flex w-full items-center gap-2 font-semibold text-[11px] text-muted-foreground/70 uppercase tracking-wider hover:text-foreground'>
              <ChevronRight className='h-3 w-3 shrink-0 transition-transform group-data-[state=open]:rotate-90' />
              <span className='shrink-0'>Expected</span>
              <span className='min-w-0 truncate font-normal text-[10px] normal-case tracking-normal opacity-50 group-data-[state=open]:hidden'>
                {testCase.expected.slice(0, 80)}
                {testCase.expected.length > 80 ? "..." : ""}
              </span>
              <span className='ml-auto shrink-0'>
                <CopyButton text={testCase.expected} />
              </span>
            </CollapsibleTrigger>
            <CollapsibleContent className='mt-1.5'>
              <TruncatedText text={testCase.expected} className='flex-1' />
            </CollapsibleContent>
          </Collapsible>
        )}

        {/* Tool */}
        {testCase.tool && (
          <div>
            <SectionHeader label='Tool' className='mb-1' />
            <p className='text-sm'>{testCase.tool}</p>
          </div>
        )}

        {/* Historical: Evaluation summary — case-level aggregate from historical data */}
        {isViewingHistorical && historicalCase && (hasJudgeReasoning || hasJudgeErrors) && (
          <Collapsible defaultOpen={false}>
            <CollapsibleTrigger className='group flex w-full items-center gap-2 font-semibold text-[11px] text-muted-foreground/70 uppercase tracking-wider hover:text-foreground'>
              <ChevronRight className='h-3 w-3 shrink-0 transition-transform group-data-[state=open]:rotate-90' />
              <span className='shrink-0'>Evaluation summary</span>
              <span className='font-normal text-[10px] normal-case tracking-normal group-data-[state=open]:hidden'>
                {hasJudgeErrors ? (
                  <span className='text-destructive/60'>judge failed</span>
                ) : (
                  <span className='opacity-50'>
                    {historicalCase.passing_runs}/{historicalCase.total_runs} runs passing
                  </span>
                )}
              </span>
            </CollapsibleTrigger>
            <CollapsibleContent className='mt-1.5 space-y-2'>
              {hasJudgeReasoning && (
                <p className='whitespace-pre-wrap rounded bg-muted p-2 text-xs'>
                  {historicalCase.judge_reasoning?.join("\n")}
                </p>
              )}
              {hasJudgeErrors && (
                <ErrorAlert
                  title='Judge failed to execute'
                  message={judgeErrors.map((err, i) => <p key={i}>{err}</p>)}
                ></ErrorAlert>
              )}
            </CollapsibleContent>
          </Collapsible>
        )}

        {/* Human review */}
        {hasResult && (
          <div className='border-t pt-4'>
            <div className='mb-2 flex items-center justify-between'>
              <span className='font-semibold text-[11px] text-muted-foreground/70 uppercase tracking-wider'>
                Human review
              </span>
              {judgeDisagreement && (
                <span className='flex items-center gap-1 text-warning text-xs'>
                  <TriangleAlert className='h-3 w-3' />
                  Disagrees with judge ({judgeVerdict})
                </span>
              )}
            </div>
            <CanWorkspaceEditor>
              <div className='flex gap-1.5'>
                <Button
                  variant={humanVerdict === "pass" ? "default" : "outline"}
                  size='sm'
                  className='h-7 gap-1 text-xs'
                  onClick={() => onSetHumanOverride(humanVerdict === "pass" ? null : "pass")}
                >
                  <ThumbsUp className='h-3 w-3' />
                  Pass
                </Button>
                <Button
                  variant={humanVerdict === "fail" ? "destructive" : "outline"}
                  size='sm'
                  className='h-7 gap-1 text-xs'
                  onClick={() => onSetHumanOverride(humanVerdict === "fail" ? null : "fail")}
                >
                  <ThumbsDown className='h-3 w-3' />
                  Fail
                </Button>
              </div>
            </CanWorkspaceEditor>
          </div>
        )}
      </div>
    </div>
  );
};

// --- Page ---

const TestFileDetailPage: React.FC = () => {
  const { pathb64 } = useParams<{ pathb64: string }>();
  const [searchParams, setSearchParams] = useSearchParams();
  const location = useLocation();
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { data: testFile, isLoading } = useTestFile(pathb64 ?? "", !!pathb64);
  const { runCase, getCase, stopFile, caseMap } = useTestFileResults();

  // Run history
  const { data: runs } = useTestRuns(pathb64 ?? "", !!pathb64);
  const deleteRun = useDeleteTestRun();
  const createRun = useCreateTestRun();

  // Selected run index — always points to a persisted run
  const urlRunIndex = searchParams.get("run_index");
  const [selectedRunIndex, setSelectedRunIndex] = useState<number | null>(
    urlRunIndex !== null ? Number(urlRunIndex) : null
  );

  // Auto-select latest run on load
  useEffect(() => {
    if (runs && runs.length > 0 && selectedRunIndex === null && urlRunIndex === null) {
      setSelectedRunIndex(runs[0].run_index);
    }
  }, [runs, selectedRunIndex, urlRunIndex]);

  const handleSelectRun = (runIndex: number) => {
    setSelectedRunIndex(runIndex);
    setSearchParams(
      (prev) => {
        prev.set("run_index", String(runIndex));
        return prev;
      },
      { replace: true }
    );
  };

  const { data: historicalRun } = useTestRunDetail(
    pathb64 ?? "",
    selectedRunIndex,
    selectedRunIndex !== null && !!pathb64
  );

  const isViewingHistorical = selectedRunIndex !== null;

  const [selectedCaseIndex, setSelectedCaseIndex] = useState<number>(0);
  const [selectedAttemptIndex, setSelectedAttemptIndex] = useState<number>(0);

  const handleSelectCase = useCallback((index: number) => {
    setSelectedCaseIndex(index);
    setSelectedAttemptIndex(0);
  }, []);

  // Hash navigation
  useEffect(() => {
    if (!location.hash) return;
    const match = location.hash.match(/^#case-(\d+)$/);
    if (match) {
      handleSelectCase(parseInt(match[1], 10));
    }
  }, [location.hash, handleSelectCase]);

  // Auto-select first failing attempt when results arrive for selected case
  const selectedCaseState = getCase(projectId, branchName, pathb64 ?? "", selectedCaseIndex);
  const prevResultRef = useRef<typeof selectedCaseState.result>(null);
  const prevCaseIndexRef = useRef<number>(selectedCaseIndex);

  useEffect(() => {
    if (prevCaseIndexRef.current !== selectedCaseIndex) {
      prevCaseIndexRef.current = selectedCaseIndex;
      prevResultRef.current = null;
      return;
    }
    if (!selectedCaseState.result) return;
    if (prevResultRef.current === null) {
      const records = selectedCaseState.result.metrics.flatMap((m) =>
        m.type !== MetricKind.Recall ? m.records : []
      );
      const firstFail = records.findIndex((r) => isErrorRecord(r) || r.score < PASS_THRESHOLD);
      if (firstFail >= 0) setSelectedAttemptIndex(firstFail);
    }
    prevResultRef.current = selectedCaseState.result;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedCaseIndex, selectedCaseState.result]);

  const { data: humanVerdictsList } = useHumanVerdicts(pathb64 ?? "", selectedRunIndex);
  const setHumanVerdictMutation = useSetHumanVerdict();

  const humanOverrides = useMemo(() => {
    const map = new Map<number, HumanVerdict>();
    if (humanVerdictsList) {
      for (const v of humanVerdictsList) {
        map.set(v.case_index, v.verdict as HumanVerdict);
      }
    }
    return map;
  }, [humanVerdictsList]);

  const setHumanOverride = (index: number, verdict: HumanVerdict | null) => {
    if (!pathb64 || selectedRunIndex === null) return;
    setHumanVerdictMutation.mutate({
      pathb64,
      runIndex: selectedRunIndex,
      caseIndex: index,
      verdict
    });
  };

  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");

  // Historical case map
  const histMap = useMemo(() => {
    const map = new Map<number, TestRunCaseResult>();
    if (isViewingHistorical && historicalRun) {
      for (const c of historicalRun.cases) {
        map.set(c.case_index, c);
      }
    }
    return map;
  }, [isViewingHistorical, historicalRun]);

  // Filtered + sorted cases (issues first)
  const filteredCases = useMemo(() => {
    if (!testFile) return [];
    return testFile.cases
      .map((tc, i) => ({
        ...tc,
        originalIndex: i,
        historicalCase: histMap.get(i) ?? null
      }))
      .filter((tc) => {
        if (search) {
          const q = search.toLowerCase();
          const hit =
            tc.prompt.toLowerCase().includes(q) ||
            (tc.expected?.toLowerCase().includes(q) ?? false) ||
            tc.tags.some((t) => t.toLowerCase().includes(q));
          if (!hit) return false;
        }
        if (statusFilter !== "all") {
          const agentV = isViewingHistorical
            ? tc.historicalCase
              ? getHistoricalVerdict(tc.historicalCase.verdict)
              : "not_run"
            : getLiveCaseVerdict(getCase(projectId, branchName, pathb64 ?? "", tc.originalIndex));
          const verdict = applyHumanOverride(agentV, humanOverrides.get(tc.originalIndex));
          if (verdict !== statusFilter) return false;
        }
        return true;
      })
      .sort((a, b) => {
        const ava = isViewingHistorical
          ? a.historicalCase
            ? getHistoricalVerdict(a.historicalCase.verdict)
            : "not_run"
          : getLiveCaseVerdict(getCase(projectId, branchName, pathb64 ?? "", a.originalIndex));
        const va = applyHumanOverride(ava, humanOverrides.get(a.originalIndex));
        const avb = isViewingHistorical
          ? b.historicalCase
            ? getHistoricalVerdict(b.historicalCase.verdict)
            : "not_run"
          : getLiveCaseVerdict(getCase(projectId, branchName, pathb64 ?? "", b.originalIndex));
        const vb = applyHumanOverride(avb, humanOverrides.get(b.originalIndex));
        const w = verdictWeight(va) - verdictWeight(vb);
        if (w !== 0) return w;
        return a.originalIndex - b.originalIndex;
      });
  }, [
    testFile,
    search,
    statusFilter,
    projectId,
    branchName,
    pathb64,
    histMap,
    isViewingHistorical,
    getCase,
    humanOverrides
  ]);

  const handleCaseNavigate = useCallback(
    (dir: -1 | 1) => {
      const currentIdx = filteredCases.findIndex((c) => c.originalIndex === selectedCaseIndex);
      const nextIdx = currentIdx + dir;
      if (nextIdx >= 0 && nextIdx < filteredCases.length) {
        handleSelectCase(filteredCases[nextIdx].originalIndex);
      }
    },
    [filteredCases, selectedCaseIndex, handleSelectCase]
  );

  // File-level aggregate stats
  const fileStats = useMemo<FileStatsData | null>(() => {
    if (!testFile) return null;

    if (isViewingHistorical && historicalRun) {
      const cases = historicalRun.cases;
      if (cases.length === 0) return null;
      const passing = cases.filter((c) => {
        const av = getHistoricalVerdict(c.verdict);
        return applyHumanOverride(av, humanOverrides.get(c.case_index)) === "pass";
      }).length;
      const flaky = cases.filter((c) => {
        const av = getHistoricalVerdict(c.verdict);
        return applyHumanOverride(av, humanOverrides.get(c.case_index)) === "flaky";
      }).length;
      const avgScore =
        cases.reduce((s, c) => {
          const hv = humanOverrides.get(c.case_index);
          if (hv === "pass") return s + 1;
          if (hv === "fail") return s + 0;
          return s + c.score;
        }, 0) / cases.length;
      const durCases = cases.filter((c) => c.avg_duration_ms != null);
      const avgDurationMs =
        durCases.length > 0
          ? durCases.reduce((s, c) => s + (c.avg_duration_ms ?? 0), 0) / durCases.length
          : null;
      const totalTokens = cases.reduce(
        (s, c) => s + (c.input_tokens ?? 0) + (c.output_tokens ?? 0),
        0
      );
      return {
        totalCases: testFile.cases.length,
        completedCases: cases.length,
        passingCases: passing,
        flakyCases: flaky,
        avgScore,
        avgDurationMs,
        totalTokens: totalTokens > 0 ? totalTokens : null
      };
    }

    // Live mode — use caseMap directly so this memo recomputes when any case changes
    const prefix = createCasePrefix(projectId, branchName, pathb64 ?? "");
    const casesMap = new Map<number, TestCaseState>();
    for (const [key, value] of caseMap) {
      if (key.startsWith(prefix)) {
        const index = parseInt(key.slice(prefix.length), 10);
        if (!Number.isNaN(index)) casesMap.set(index, value);
      }
    }
    let completedCases = 0,
      passingCases = 0,
      flakyCases = 0,
      totalScore = 0,
      totalDuration = 0,
      durationCount = 0,
      totalTokens = 0;

    for (let i = 0; i < testFile.cases.length; i++) {
      const state = casesMap.get(i);
      if (!state?.result) continue;
      completedCases++;
      const agentV = getLiveCaseVerdict(state);
      const hv = humanOverrides.get(i);
      const verdict = applyHumanOverride(agentV, hv);
      if (verdict === "pass") passingCases++;
      if (verdict === "flaky") flakyCases++;
      if (hv === "pass") {
        totalScore += 1;
      } else if (hv === "fail") {
        totalScore += 0;
      } else {
        const { passing, total } = getConsistency(state.result.metrics);
        if (total > 0) totalScore += passing / total;
      }
      const records = state.result.metrics.flatMap((m) =>
        m.type !== MetricKind.Recall ? m.records : []
      );
      const successRecords = records.filter((r) => !isErrorRecord(r));
      if (successRecords.length > 0) {
        totalDuration +=
          successRecords.reduce((s, r) => s + r.duration_ms, 0) / successRecords.length;
        durationCount++;
      }
      totalTokens += successRecords.reduce((s, r) => s + r.input_tokens + r.output_tokens, 0);
    }

    if (completedCases === 0) return null;
    return {
      totalCases: testFile.cases.length,
      completedCases,
      passingCases,
      flakyCases,
      avgScore: totalScore / completedCases,
      avgDurationMs: durationCount > 0 ? totalDuration / durationCount : null,
      totalTokens: totalTokens > 0 ? totalTokens : null
    };
  }, [
    testFile,
    isViewingHistorical,
    historicalRun,
    projectId,
    branchName,
    pathb64,
    caseMap,
    humanOverrides
  ]);

  const isFileRunning = useMemo(() => {
    if (!testFile) return false;
    const prefix = createCasePrefix(projectId, branchName, pathb64 ?? "");
    for (const [key, cs] of caseMap) {
      if (key.startsWith(prefix)) {
        if (cs.state === EvalEventState.Started || cs.state === EvalEventState.Progress)
          return true;
      }
    }
    return false;
  }, [testFile, projectId, branchName, pathb64, caseMap]);

  if (!pathb64) return null;

  if (isLoading) {
    return (
      <div className='flex h-full flex-col'>
        <div className='p-4'>
          <Skeleton className='mb-4 h-8 w-64' />
          <div className='space-y-3'>
            {Array.from({ length: 3 }).map((_, i) => (
              <Skeleton key={i} className='h-16 w-full' />
            ))}
          </div>
        </div>
      </div>
    );
  }

  if (!testFile) {
    return (
      <div className='flex h-full items-center justify-center text-muted-foreground text-sm'>
        Test file not found
      </div>
    );
  }

  const handleRunAll = async () => {
    try {
      const run = await createRun.mutateAsync({ pathb64 });
      handleSelectRun(run.run_index);
      testFile.cases.forEach((_, index) => {
        runCase(projectId, branchName, pathb64, index, run.run_index);
      });
    } catch (error) {
      // Fallback: run without a persisted run — and say so, because without a
      // `run_index` the server saves none of these results.
      console.error("Failed to create test run:", error);
      toast.error("Failed to create a test run", {
        id: "test-run-not-saved",
        description: "The tests will still run, but their results won't be saved."
      });
      testFile.cases.forEach((_, index) => {
        runCase(projectId, branchName, pathb64, index);
      });
    }
  };

  const currentRun = runs?.find((r) => r.run_index === selectedRunIndex);
  const storageKey = `ide:split:test-detail:${pathb64}`;
  const selectedTestCase = testFile.cases[selectedCaseIndex] ?? null;
  const selectedHistoricalCase = histMap.get(selectedCaseIndex) ?? null;
  const filteredCasePosition = filteredCases.findIndex(
    (c) => c.originalIndex === selectedCaseIndex
  );

  return (
    <div className='flex h-full flex-col'>
      {/* Header */}
      <PageHeader
        icon={FileText}
        title={testFile.name ?? "Test Details"}
        actions={
          <>
            {runs && runs.length > 0 && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant='secondary' size='sm' className='gap-1.5 text-xs'>
                    <History className='h-3.5 w-3.5' />
                    {currentRun ? formatRunLabel(currentRun) : "Select run"}
                    <ChevronDown className='h-3 w-3 opacity-50' />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align='end' className='w-72'>
                  <DropdownMenuLabel className='text-xs'>Run History</DropdownMenuLabel>
                  <DropdownMenuSeparator />
                  {runs.map((run) => (
                    <DropdownMenuItem
                      key={run.run_index}
                      className='flex items-center gap-1 text-xs'
                      onSelect={() => handleSelectRun(run.run_index)}
                    >
                      <span
                        className={cn(
                          "flex-1 truncate",
                          selectedRunIndex === run.run_index && "font-semibold"
                        )}
                      >
                        {formatRunLabel(run)}
                      </span>
                      {run.score !== null ? (
                        (() => {
                          const pct = Math.round(run.score * 100);
                          return (
                            <Badge
                              variant='outline'
                              className={cn("ml-1 shrink-0 text-[10px]", scoreColorClass(pct))}
                            >
                              {pct}%
                            </Badge>
                          );
                        })()
                      ) : (
                        <Badge
                          variant='outline'
                          className='ml-1 shrink-0 gap-1 border-destructive/50 text-[10px] text-destructive'
                        >
                          <AlertCircle className='h-3 w-3 text-destructive' />
                          Failed
                        </Badge>
                      )}
                      <CanWorkspaceAdmin>
                        <Button
                          variant='ghost'
                          size='icon'
                          className='ml-1 h-5 w-5 shrink-0 opacity-60 hover:opacity-100'
                          onClick={(e) => {
                            e.stopPropagation();
                            e.preventDefault();
                            deleteRun.mutate({
                              pathb64,
                              runIndex: run.run_index
                            });
                            if (selectedRunIndex === run.run_index && runs.length > 1) {
                              const other = runs.find((r) => r.run_index !== run.run_index);
                              if (other) handleSelectRun(other.run_index);
                            }
                          }}
                        >
                          <Trash2 className='h-3 w-3' />
                        </Button>
                      </CanWorkspaceAdmin>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
            <Link to={ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.FILES.FILE(pathb64)}>
              <Button variant='outline' size='sm' className='gap-1'>
                <Pencil className='h-3 w-3' />
                Edit
              </Button>
            </Link>
            <CanWorkspaceEditor>
              {isFileRunning ? (
                <Button
                  variant='destructive'
                  size='sm'
                  className='gap-1'
                  onClick={() => stopFile(projectId, branchName, pathb64)}
                >
                  <Square className='h-3 w-3 fill-current' />
                  Stop
                </Button>
              ) : (
                <Button variant='default' size='sm' className='gap-1' onClick={handleRunAll}>
                  <Play className='h-3 w-3' />
                  Run All
                </Button>
              )}
            </CanWorkspaceEditor>
          </>
        }
      />

      {/* Summary strip */}
      <SummaryStrip
        stats={fileStats}
        judgeModel={testFile.settings.judge_model ?? null}
        runs={testFile.settings.runs}
      />

      {/* Failed run banner */}
      {isViewingHistorical &&
        historicalRun &&
        historicalRun.cases.length === 0 &&
        !isFileRunning && (
          <div className='flex items-center gap-2 border-destructive/30 border-b bg-destructive/5 px-4 py-2 text-sm'>
            <AlertCircle className='h-3.5 w-3.5 shrink-0 text-destructive' />
            <span className='font-medium text-destructive'>Run failed</span>
            <span className='text-[11px] text-muted-foreground'>
              No results were recorded — something went wrong
            </span>
          </div>
        )}

      {/* Main 2-pane layout */}
      <ResizablePanelGroup
        direction='horizontal'
        autoSaveId={storageKey}
        className='flex-1 overflow-hidden'
      >
        {/* Left pane: case list */}
        <ResizablePanel defaultSize={35} minSize={20}>
          <div className='flex h-full flex-col'>
            {/* Search + filter */}
            <div className='space-y-1.5 border-b p-2'>
              <div className='relative'>
                <Search className='absolute top-1/2 left-2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground' />
                <Input
                  placeholder='Search...'
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  className='h-7 pl-7 text-xs'
                />
              </div>
              <div className='flex items-center gap-0.5'>
                {(["all", "pass", "fail", "flaky"] as StatusFilter[]).map((s) => (
                  <button
                    key={s}
                    type='button'
                    onClick={() => setStatusFilter(s)}
                    className={cn(
                      "rounded px-2 py-0.5 font-medium text-xs transition-colors",
                      statusFilter === s
                        ? "bg-primary text-primary-foreground"
                        : "text-muted-foreground hover:bg-muted hover:text-foreground"
                    )}
                  >
                    {s === "all" ? "All" : s.charAt(0).toUpperCase() + s.slice(1)}
                  </button>
                ))}
                <span className='ml-auto text-muted-foreground text-xs'>
                  {filteredCases.length}/{testFile.cases.length}
                </span>
              </div>
            </div>

            {/* Case rows */}
            <div className='flex-1 overflow-y-auto'>
              {filteredCases.length === 0 && (
                <p className='p-4 text-center text-muted-foreground text-xs'>
                  No cases match the current filter.
                </p>
              )}
              {filteredCases.map((tc) => {
                const index = tc.originalIndex;
                const cs = getCase(projectId, branchName, pathb64, index);
                const agentVerdict = isViewingHistorical
                  ? tc.historicalCase
                    ? getHistoricalVerdict(tc.historicalCase.verdict)
                    : "not_run"
                  : getLiveCaseVerdict(cs);
                const verdict = applyHumanOverride(agentVerdict, humanOverrides.get(index));
                const score = isViewingHistorical
                  ? tc.historicalCase
                    ? Math.round(tc.historicalCase.score * 100)
                    : null
                  : cs.result?.metrics[0]?.score != null
                    ? Math.round(cs.result.metrics[0].score * 100)
                    : null;
                const isSelected = index === selectedCaseIndex;

                return (
                  <button
                    key={index}
                    type='button'
                    onClick={() => handleSelectCase(index)}
                    className={cn(
                      "flex w-full items-center gap-2 border-b px-3 py-2 text-left transition-colors",
                      isSelected
                        ? "border-l-2 border-l-primary bg-muted"
                        : "border-l-2 border-l-transparent hover:bg-muted/50"
                    )}
                  >
                    <VerdictIcon verdict={verdict} className='h-3.5 w-3.5' />
                    <div className='min-w-0 flex-1'>
                      <p className='truncate text-xs'>{tc.prompt}</p>
                      {tc.tags.length > 0 && (
                        <div className='mt-0.5 flex gap-0.5 overflow-hidden'>
                          {tc.tags.slice(0, 3).map((tag) => (
                            <span
                              key={tag}
                              className='shrink-0 rounded border border-border/50 px-1 py-px text-[9px] text-muted-foreground/60'
                            >
                              {tag}
                            </span>
                          ))}
                        </div>
                      )}
                    </div>
                    {score !== null && (
                      <span
                        className={cn(
                          "shrink-0 font-medium text-xs tabular-nums",
                          score >= 50 ? "text-success" : "text-destructive"
                        )}
                      >
                        {score}%
                      </span>
                    )}
                  </button>
                );
              })}
            </div>
          </div>
        </ResizablePanel>

        <ResizableHandle withHandle />

        {/* Right pane: case detail */}
        <ResizablePanel defaultSize={65} minSize={30}>
          {selectedTestCase ? (
            <CaseDetailPanel
              index={selectedCaseIndex}
              filteredPosition={filteredCasePosition >= 0 ? filteredCasePosition : 0}
              totalCases={filteredCases.length}
              testCase={selectedTestCase}
              caseState={selectedCaseState}
              historicalCase={selectedHistoricalCase}
              isViewingHistorical={isViewingHistorical}
              selectedAttemptIndex={selectedAttemptIndex}
              onSelectAttempt={setSelectedAttemptIndex}
              humanOverride={humanOverrides.get(selectedCaseIndex) ?? null}
              onSetHumanOverride={(v) => setHumanOverride(selectedCaseIndex, v)}
              onRun={async () => {
                if (selectedRunIndex !== null) {
                  // Reuse the active run so the result is persisted under the same record.
                  runCase(projectId, branchName, pathb64, selectedCaseIndex, selectedRunIndex);
                } else {
                  try {
                    const run = await createRun.mutateAsync({ pathb64 });
                    handleSelectRun(run.run_index);
                    runCase(projectId, branchName, pathb64, selectedCaseIndex, run.run_index);
                  } catch (error) {
                    console.error("Failed to create test run:", error);
                    toast.error("Failed to create a test run", {
                      id: "test-run-not-saved",
                      description: "The test will still run, but its result won't be saved."
                    });
                    runCase(projectId, branchName, pathb64, selectedCaseIndex);
                  }
                }
              }}
              onNavigate={handleCaseNavigate}
            />
          ) : (
            <div className='flex h-full items-center justify-center text-muted-foreground text-sm'>
              Select a case to view details
            </div>
          )}
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
};

export default TestFileDetailPage;
