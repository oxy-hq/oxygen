import { useQueryClient } from "@tanstack/react-query";
import type { EChartsOption } from "echarts";
import { getInstanceByDom, init } from "echarts";
import {
  AlertCircle,
  CheckCircle2,
  ChevronDown,
  Clock,
  FlaskConical,
  History,
  Layers,
  Pencil,
  Play,
  Plus,
  ShieldCheck,
  Square,
  XCircle,
  Zap
} from "lucide-react";
import type React from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useResizeDetector } from "react-resize-detector";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { resolveColor, resolveColorWithAlpha } from "@/components/Echarts/resolveColor";
import theme from "@/components/Echarts/theme.json";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/shadcn/card";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { FieldError } from "@/components/ui/shadcn/field";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Spinner } from "@/components/ui/shadcn/spinner";
import queryKeys from "@/hooks/api/queryKey";
import useTestFile from "@/hooks/api/tests/useTestFile";
import useTestFiles from "@/hooks/api/tests/useTestFiles";
import {
  useCreateTestProjectRun,
  useDeleteTestProjectRun,
  useTestProjectRuns
} from "@/hooks/api/tests/useTestProjectRuns";
import { useCreateTestRun, useTestRunDetail } from "@/hooks/api/tests/useTestRuns";
import { useCreateTestFile } from "@/hooks/useCreateTestFile";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import PageHeader from "@/pages/ide/components/PageHeader";
import type { TestFileConfig } from "@/services/api/testFiles";
import type { TestProjectRunInfo } from "@/services/api/testProjectRuns";
import type { TestRunCaseResult } from "@/services/api/testRuns";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useTestFileResults, {
  createCaseKey,
  type TestCaseResult,
  type TestCaseState
} from "@/stores/useTestFileResults";
import {
  EvalEventState,
  type Record as EvalRecord,
  MetricKind,
  type MetricValue
} from "@/types/eval";

// --- Types ---

interface HistoricalFileStatsEntry {
  score: number;
  passing: number;
  scored: number;
  avgDuration: number | null;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalRuns: number;
  passingRuns: number;
  verdictPass: number;
  verdictFail: number;
  verdictFlaky: number;
}

// --- Helpers ---

const scoreClass = (pct: number) =>
  pct >= 80
    ? "border-success text-success"
    : pct >= 50
      ? "border-warning text-warning"
      : "border-destructive text-destructive";

const formatDuration = (ms: number) => {
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms)}ms`;
};

const formatTokens = (n: number) => {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
};

const formatRunLabel = (run: TestProjectRunInfo) => {
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

const getScorePercent = (metrics: MetricValue[]) => {
  if (metrics.length === 0) return null;
  return Math.round(metrics[0].score * 100);
};

const getCaseMetrics = (result: TestCaseResult) => {
  const failures = result.stats.total_attempted - result.stats.answered;
  const records = result.metrics.flatMap((m) => (m.type === MetricKind.Recall ? [] : m.records));
  const successRecords = records.filter((r) => !isErrorRecord(r));
  const avgDuration =
    successRecords.length > 0
      ? successRecords.reduce((s, r) => s + r.duration_ms, 0) / successRecords.length
      : null;
  const totalInputTokens = successRecords.reduce((s, r) => s + r.input_tokens, 0);
  const totalOutputTokens = successRecords.reduce((s, r) => s + r.output_tokens, 0);
  return { failures, avgDuration, totalInputTokens, totalOutputTokens };
};

type CaseVerdict = "pass" | "fail" | "flaky" | "running" | "error" | "not_run";

const getCaseVerdict = (caseState: TestCaseState | undefined): CaseVerdict => {
  if (!caseState || caseState.state === null) return "not_run";
  if (caseState.error) return "error";
  if (caseState.state === EvalEventState.Started || caseState.state === EvalEventState.Progress)
    return "running";
  if (!caseState.result) return "not_run";
  const { passing, total } = getConsistency(caseState.result.metrics);
  if (total === 0) return "not_run";
  if (passing === total) return "pass";
  if (passing === 0) return "fail";
  return "flaky";
};

const getHistoricalVerdict = (c: TestRunCaseResult): CaseVerdict => {
  if (c.human_verdict === "pass") return "pass";
  if (c.human_verdict === "fail") return "fail";
  if (c.human_verdict === "ambiguous") return "flaky";
  if (c.verdict === "pass") return "pass";
  if (c.verdict === "fail") return "fail";
  if (c.verdict === "flaky") return "flaky";
  return "not_run";
};

const getEffectiveScore = (c: TestRunCaseResult): number => {
  if (c.human_verdict === "pass") return 1;
  if (c.human_verdict === "fail") return 0;
  return c.score;
};

const VerdictIcon: React.FC<{ verdict: CaseVerdict; className?: string }> = ({
  verdict,
  className
}) => {
  switch (verdict) {
    case "pass":
      return <CheckCircle2 className={cn("h-3.5 w-3.5 text-success", className)} />;
    case "fail":
      return <XCircle className={cn("h-3.5 w-3.5 text-destructive", className)} />;
    case "flaky":
      return <XCircle className={cn("h-3.5 w-3.5 text-warning", className)} />;
    case "error":
      return <XCircle className={cn("h-3.5 w-3.5 text-destructive", className)} />;
    case "running":
      return <Spinner className={cn("size-3", "text-primary", className)} />;
    default:
      return (
        <div
          className={cn("h-3.5 w-3.5 rounded-full border border-muted-foreground/30", className)}
        />
      );
  }
};

const verdictLabel = (verdict: CaseVerdict) => {
  switch (verdict) {
    case "pass":
      return "passing";
    case "fail":
      return "failing";
    case "flaky":
      return "flaky";
    case "running":
      return "running";
    case "error":
      return "error";
    default:
      return "not run";
  }
};

const getScoreVariant = (score: number): "success" | "warning" | "danger" => {
  if (score >= 80) return "success";
  if (score >= 50) return "warning";
  return "danger";
};

const variantIconBg = (variant: "default" | "success" | "warning" | "danger") => {
  switch (variant) {
    case "success":
      return "bg-success/10 text-success";
    case "warning":
      return "bg-warning/10 text-warning";
    case "danger":
      return "bg-destructive/10 text-destructive";
    default:
      return "bg-primary/10 text-primary";
  }
};

// --- DashboardStatsCard ---

const DashboardStatsCard: React.FC<{
  title: string;
  value: string | number;
  subtitle: string;
  icon: React.ReactNode;
  variant?: "default" | "success" | "warning" | "danger";
}> = ({ title, value, subtitle, icon, variant = "default" }) => (
  <Card className='overflow-hidden'>
    <CardContent className='p-3'>
      <div className='flex items-center justify-between gap-2'>
        <div className='min-w-0 flex-1'>
          <p className='font-medium text-[11px] text-muted-foreground'>{title}</p>
          <p className='truncate font-bold text-lg leading-tight tracking-tight'>{value}</p>
          <p className='truncate text-[11px] text-muted-foreground'>{subtitle}</p>
        </div>
        <div className={cn("shrink-0 rounded-md p-1.5", variantIconBg(variant))}>{icon}</div>
      </div>
    </CardContent>
  </Card>
);

// --- Verdict Donut Chart ---

const VerdictDonutChart: React.FC<{
  pass: number;
  fail: number;
  flaky: number;
}> = ({ pass, fail, flaky }) => {
  const chartRef = useRef<HTMLDivElement>(null);

  const onResize = useCallback(() => {
    if (chartRef.current) {
      getInstanceByDom(chartRef.current)?.resize();
    }
  }, []);

  useResizeDetector({ targetRef: chartRef, onResize });

  useEffect(() => {
    if (!chartRef.current) return;
    const chart = init(chartRef.current, theme);
    return () => {
      chart.dispose();
    };
  }, []);

  const total = pass + fail + flaky;
  const data =
    total > 0
      ? [
          { value: pass, name: "Pass", color: resolveColor("--success") },
          { value: fail, name: "Fail", color: resolveColor("--destructive") },
          { value: flaky, name: "Flaky", color: resolveColor("--warning") }
        ].filter((d) => d.value > 0)
      : [
          {
            value: 1,
            name: "Not run",
            color: resolveColor("--zinc-300")
          }
        ];

  useEffect(() => {
    if (!chartRef.current) return;
    const chart = getInstanceByDom(chartRef.current);
    const options: EChartsOption = {
      tooltip: { trigger: "item", formatter: "{b}: {c} ({d}%)", appendToBody: true },
      series: [
        {
          type: "pie",
          radius: ["48%", "80%"],
          center: ["50%", "50%"],
          avoidLabelOverlap: false,
          itemStyle: {
            borderRadius: 2,
            borderColor: "transparent",
            borderWidth: 2
          },
          label: { show: false },
          emphasis: {
            label: { show: false },
            itemStyle: { color: "inherit" }
          },
          labelLine: { show: false },
          data: data.map((d) => ({
            value: d.value,
            name: d.name,
            itemStyle: { color: d.color }
          }))
        }
      ]
    };
    chart?.setOption(options, true);
    chart?.resize();
  }, [data]);

  return (
    <div className='flex shrink-0 flex-col items-center gap-2'>
      <div ref={chartRef} className='h-[72px] w-[72px]' />
      <div className='flex flex-wrap justify-center gap-x-2 gap-y-0.5'>
        {data.map((d) => (
          <span key={d.name} className='flex items-center gap-1 text-[10px] text-muted-foreground'>
            <span
              className='inline-block h-2 w-2 shrink-0 rounded-sm'
              style={{ backgroundColor: d.color }}
            />
            {d.name}
          </span>
        ))}
      </div>
    </div>
  );
};

// --- Aggregate Trend Bar Chart ---

interface ProjectRunTrendPoint {
  label: string;
  shortLabel: string;
  score: number;
  failed?: boolean;
  fileBreakdown: Array<{ name: string; score: number }>;
}

const TREND_CHART_NARROW_WIDTH = 560;

const TrendBarChart: React.FC<{
  points: ProjectRunTrendPoint[];
  selectedIdx?: number;
  onBarClick?: (idx: number) => void;
}> = ({ points, selectedIdx, onBarClick }) => {
  const chartRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number>(0);
  const onBarClickRef = useRef(onBarClick);
  onBarClickRef.current = onBarClick;
  const pointsLengthRef = useRef(points.length);
  pointsLengthRef.current = points.length;

  const onResize = useCallback(() => {
    if (chartRef.current) {
      const w = chartRef.current.offsetWidth;
      setWidth(w);
      getInstanceByDom(chartRef.current)?.resize();
    }
  }, []);

  useResizeDetector({ targetRef: chartRef, onResize });

  useEffect(() => {
    if (!chartRef.current) return;
    const chart = init(chartRef.current, theme);
    chart.getZr().on("mousemove", (params: { offsetX: number }) => {
      const xIndex = chart.convertFromPixel({ xAxisIndex: 0 }, params.offsetX);
      const inRange = typeof xIndex === "number" && xIndex >= 0 && xIndex < pointsLengthRef.current;
      chart.getZr().setCursorStyle(inRange ? "pointer" : "default");
    });
    chart.getZr().on("mouseout", () => {
      chart.getZr().setCursorStyle("default");
    });
    chart.getZr().on("click", (params: { offsetX: number; offsetY: number }) => {
      const xIndex = chart.convertFromPixel({ xAxisIndex: 0 }, params.offsetX);
      if (typeof xIndex === "number" && xIndex >= 0 && xIndex < pointsLengthRef.current) {
        onBarClickRef.current?.(Math.round(xIndex));
      }
    });
    onResize();
    return () => {
      chart.dispose();
    };
  }, [onResize]);

  useEffect(() => {
    if (!chartRef.current) return;
    const chart = getInstanceByDom(chartRef.current);

    const labels = points.map((p) => p.shortLabel);
    const isNarrow = width > 0 && width < TREND_CHART_NARROW_WIDTH;
    const labelInterval = isNarrow && labels.length > 2 ? 1 : 0;

    // Failed bars use a fixed short value so they appear as a stub
    const FAILED_BAR_HEIGHT = 8;

    const options: EChartsOption = {
      tooltip: {
        trigger: "axis",
        axisPointer: { type: "shadow" },
        formatter: (params: unknown) => {
          const items = params as Array<{ dataIndex: number; value: number }>;
          const idx = items[0]?.dataIndex ?? 0;
          const point = points[idx];
          if (!point) return "";
          const header = `<div style="font-weight:600;margin-bottom:4px">${point.label}</div>`;
          if (point.failed) {
            return (
              header +
              `<div style="font-size:12px;color:var(--destructive)">Run failed — no results recorded</div>`
            );
          }
          const overall = `<div style="font-size:13px;font-weight:bold">${Math.round(point.score * 100)}% pass rate</div>`;
          const breakdown =
            point.fileBreakdown.length > 0
              ? `<div style="margin-top:6px;font-size:11px">${point.fileBreakdown
                  .map(
                    (f) =>
                      `<div style="display:flex;justify-content:space-between;gap:16px"><span>${f.name}</span><span>${Math.round(f.score * 100)}%</span></div>`
                  )
                  .join("")}</div>`
              : "";
          return header + overall + breakdown;
        }
      },
      grid: { left: 36, right: 8, top: 22, bottom: 24 },
      xAxis: {
        type: "category",
        data: labels,
        axisLabel: {
          fontSize: 10,
          rotate: 0,
          interval: labelInterval,
          hideOverlap: true
        },
        axisTick: { alignWithLabel: true }
      },
      yAxis: {
        type: "value",
        min: 0,
        max: 100,
        interval: 25,
        axisLabel: { formatter: "{value}%", fontSize: 10 }
      },
      series: [
        {
          type: "bar",
          data: points.map((p, i) => {
            const s = Math.round(p.score * 100);
            const isFailed = p.failed === true;
            const isSelected = i === selectedIdx;
            const barColor = isFailed
              ? isSelected
                ? resolveColorWithAlpha("--destructive", 0.7)
                : resolveColorWithAlpha("--destructive", 0.5)
              : isSelected
                ? s >= 80
                  ? resolveColorWithAlpha("--success", 0.8)
                  : s >= 50
                    ? resolveColorWithAlpha("--warning", 0.8)
                    : resolveColorWithAlpha("--destructive", 0.8)
                : s >= 80
                  ? resolveColor("--success")
                  : s >= 50
                    ? resolveColor("--warning")
                    : resolveColor("--destructive");
            return {
              value: isFailed ? FAILED_BAR_HEIGHT : s,
              itemStyle: {
                color: barColor,
                borderRadius: [3, 3, 0, 0],
                opacity: selectedIdx !== undefined && i !== selectedIdx ? 0.4 : 1,
                borderColor: isSelected ? resolveColor("--border") : "transparent",
                borderWidth: isSelected ? 1.5 : 0
              },
              emphasis: { itemStyle: { opacity: 0.85 } }
            };
          }),
          barMaxWidth: 40,
          label: {
            show: true,
            position: "top",
            fontSize: 11,
            fontWeight: 600,
            color: resolveColor("--border"),
            rich: {
              err: {
                color: resolveColor("--destructive"),
                fontSize: 10,
                fontWeight: 700,
                align: "center",
                width: 14,
                height: 14,
                lineHeight: 14,
                borderColor: resolveColor("--destructive"),
                borderWidth: 1.5,
                borderRadius: 8,
                padding: [0, 0, 0, 0]
              }
            },
            formatter: (params: unknown) => {
              const p = params as { dataIndex: number };
              const point = points[p.dataIndex];
              if (point?.failed) return "{err|!}";
              return `${Math.round(point?.score * 100)}%`;
            }
          }
        }
      ]
    };
    chart?.setOption(options, true);
    chart?.resize();
  }, [points, selectedIdx, width]);

  return <div ref={chartRef} className='h-[150px] w-full' />;
};

// --- Pie progress indicator ---

const PieProgress: React.FC<{
  completed: number;
  total: number;
  size?: number;
}> = ({ completed, total, size = 18 }) => {
  const percent = total > 0 ? completed / total : 0;
  const r = size / 2 - 2;
  const circumference = 2 * Math.PI * r;
  const offset = circumference * (1 - percent);
  return (
    <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className='shrink-0 -rotate-90'>
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill='none'
        strokeWidth='2.5'
        style={{ stroke: "hsl(var(--muted-foreground) / 0.25)" }}
      />
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill='none'
        strokeWidth='2.5'
        strokeLinecap='round'
        strokeDasharray={circumference}
        strokeDashoffset={offset}
        style={{
          stroke: "hsl(var(--primary))",
          transition: "stroke-dashoffset 0.5s ease"
        }}
      />
    </svg>
  );
};

// --- Main page ---

const TestsDashboardPage: React.FC = () => {
  const { data: testFiles, isLoading } = useTestFiles();
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project.id;
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const store = useTestFileResults();
  const createTestFile = useCreateTestFile();
  const createRun = useCreateTestRun();
  const createProjectRun = useCreateTestProjectRun();
  const deleteProjectRun = useDeleteTestProjectRun();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();

  // Project runs for the selector and trend chart
  const { data: projectRuns } = useTestProjectRuns();

  // Selected project run — URL param takes precedence, otherwise auto-select latest
  const urlRunId = searchParams.get("run_id");
  const [selectedProjectRunId, setSelectedProjectRunId] = useState<string | null>(urlRunId);

  const latestProjectRunId = projectRuns?.[0]?.id ?? null;
  const prevLatestRef = useRef(latestProjectRunId);

  useEffect(() => {
    const isNewRun = latestProjectRunId && latestProjectRunId !== prevLatestRef.current;
    const isFirstLoad = latestProjectRunId && !selectedProjectRunId;
    if (isNewRun || isFirstLoad) {
      setSelectedProjectRunId(latestProjectRunId);
      setSearchParams(
        (prev) => {
          prev.set("run_id", latestProjectRunId);
          return prev;
        },
        { replace: true }
      );
    }
    prevLatestRef.current = latestProjectRunId;
  }, [latestProjectRunId, selectedProjectRunId, setSearchParams]);

  const handleSelectRun = useCallback(
    (runId: string | null) => {
      setSelectedProjectRunId(runId);
      setSearchParams(
        (prev) => {
          if (runId) prev.set("run_id", runId);
          else prev.delete("run_id");
          return prev;
        },
        { replace: true }
      );
    },
    [setSearchParams]
  );

  const selectedProjectRun = useMemo(
    () => projectRuns?.find((r) => r.id === selectedProjectRunId) ?? null,
    [projectRuns, selectedProjectRunId]
  );

  const isViewingLatest = selectedProjectRunId === latestProjectRunId;

  // Effective run index per file (from selected project run)
  const effectiveRunIndexMap = useMemo(() => {
    const map = new Map<string, number | null>();
    if (testFiles && selectedProjectRun) {
      for (const file of testFiles) {
        const pb64 = encodeBase64(file.path);
        const fileScore = selectedProjectRun.file_scores.find((f) => f.source_id === file.path);
        map.set(pb64, fileScore?.run_index ?? null);
      }
    }
    return map;
  }, [testFiles, selectedProjectRun]);

  const [runNameDialogOpen, setRunNameDialogOpen] = useState(false);
  const [pendingRunName, setPendingRunName] = useState("");
  const [pendingRunPathb64, setPendingRunPathb64] = useState<string | null>(null);

  const openRunDialog = (pathb64: string | null) => {
    setPendingRunPathb64(pathb64);
    setPendingRunName("");
    setRunNameDialogOpen(true);
  };

  const handleConfirmRun = async () => {
    if (!testFiles) return;
    setRunNameDialogOpen(false);
    const name = pendingRunName.trim() || undefined;
    const filesToRun = pendingRunPathb64
      ? testFiles.filter((f) => encodeBase64(f.path) === pendingRunPathb64)
      : testFiles;

    let projectRunId: string | undefined;
    if (!pendingRunPathb64) {
      try {
        const pr = await createProjectRun.mutateAsync({ name });
        projectRunId = pr.id;
        handleSelectRun(pr.id);
      } catch (error) {
        // Continue without project run grouping — but say so: the files still run, and
        // nothing ties them together as one run afterwards.
        console.error("Failed to create project run:", error);
        toast.error("Failed to create the run", {
          description: "The tests will still run, but won't be grouped into one run."
        });
      }
    }

    for (const file of filesToRun) {
      const pathb64 = encodeBase64(file.path);
      try {
        const run = await createRun.mutateAsync({
          pathb64,
          name,
          projectRunId
        });
        for (let i = 0; i < file.case_count; i++) {
          store.runCase(projectId, branchName, pathb64, i, run.run_index);
        }
      } catch (error) {
        // The cases still run, but without a `run_index` the server persists nothing.
        // One toast id, so a failure on every file does not stack a toast per file.
        console.error("Failed to create test run:", error);
        toast.error("Failed to create a test run", {
          id: "test-run-not-saved",
          description: "The tests will still run, but their results won't be saved."
        });
        for (let i = 0; i < file.case_count; i++) {
          store.runCase(projectId, branchName, pathb64, i);
        }
      }
    }
  };

  // Historical file stats (loaded invisibly by FileStatsLoader components)
  const [historicalFileStats, setHistoricalFileStats] = useState<
    Map<string, HistoricalFileStatsEntry | null>
  >(new Map());

  const reportFileStats = useCallback((pathb64: string, stats: HistoricalFileStatsEntry | null) => {
    setHistoricalFileStats((prev) => {
      const next = new Map(prev);
      next.set(pathb64, stats);
      return next;
    });
  }, []);

  const anyRunning = useMemo(() => {
    if (!testFiles) return false;
    for (const file of testFiles) {
      const pathb64 = encodeBase64(file.path);
      for (let i = 0; i < file.case_count; i++) {
        const cs = store.caseMap.get(createCaseKey(projectId, branchName, pathb64, i));
        if (cs?.state === EvalEventState.Started || cs?.state === EvalEventState.Progress)
          return true;
      }
    }
    return false;
  }, [testFiles, projectId, branchName, store.caseMap]);

  // Live health from Zustand (used while a run is in progress)
  const liveHealth = useMemo(() => {
    if (!testFiles || testFiles.length === 0) return null;

    let totalCases = 0,
      casesWithResults = 0,
      passingCases = 0;
    let totalRuns = 0,
      passingRuns = 0;
    let totalInputTokens = 0,
      totalOutputTokens = 0;
    let durationSum = 0,
      durationCount = 0;
    let verdictPass = 0,
      verdictFail = 0,
      verdictFlaky = 0;

    for (const file of testFiles) {
      const pathb64 = encodeBase64(file.path);
      for (let i = 0; i < file.case_count; i++) {
        totalCases++;
        const cs = store.caseMap.get(createCaseKey(projectId, branchName, pathb64, i));
        if (cs?.result) {
          casesWithResults++;
          const { passing, total } = getConsistency(cs.result.metrics);
          if (total > 0) {
            totalRuns += total;
            passingRuns += passing;
            if (passing === total) {
              passingCases++;
              verdictPass++;
            } else if (passing === 0) verdictFail++;
            else verdictFlaky++;
          }
          for (const m of cs.result.metrics) {
            if (m.type !== MetricKind.Recall) {
              for (const r of m.records) {
                if (!isErrorRecord(r)) {
                  totalInputTokens += r.input_tokens;
                  totalOutputTokens += r.output_tokens;
                  if (r.duration_ms > 0) {
                    durationSum += r.duration_ms;
                    durationCount++;
                  }
                }
              }
            }
          }
        }
      }
    }

    if (casesWithResults === 0) return null;

    const score = Math.round(
      testFiles.reduce((sum, file) => {
        const pathb64 = encodeBase64(file.path);
        let fileScored = 0,
          fileTotal = 0;
        for (let i = 0; i < file.case_count; i++) {
          const cs = store.caseMap.get(createCaseKey(projectId, branchName, pathb64, i));
          if (cs?.result) {
            const s = getScorePercent(cs.result.metrics);
            if (s !== null) {
              fileTotal += s;
              fileScored++;
            }
          }
        }
        return sum + (fileScored > 0 ? fileTotal / fileScored : 0);
      }, 0) / testFiles.length
    );

    return {
      score,
      casesWithResults,
      totalCases,
      passingCases,
      totalRuns,
      passingRuns,
      avgDuration: durationCount > 0 ? durationSum / durationCount : null,
      totalTokens: totalInputTokens + totalOutputTokens,
      totalInputTokens,
      totalOutputTokens,
      verdictCounts: {
        pass: verdictPass,
        fail: verdictFail,
        flaky: verdictFlaky
      }
    };
  }, [testFiles, projectId, branchName, store.caseMap]);

  // Historical health aggregated from FileStatsLoaders
  const historicalHealth = useMemo(() => {
    if (!testFiles || testFiles.length === 0 || !selectedProjectRun) return null;
    const entries = Array.from(historicalFileStats.values()).filter(
      Boolean
    ) as HistoricalFileStatsEntry[];
    if (entries.length === 0) return null;

    const totalCases = entries.reduce((s, e) => s + e.scored, 0);
    const passingCases = entries.reduce((s, e) => s + e.passing, 0);
    const totalRuns = entries.reduce((s, e) => s + e.totalRuns, 0);
    const passingRuns = entries.reduce((s, e) => s + e.passingRuns, 0);
    const totalInputTokens = entries.reduce((s, e) => s + e.totalInputTokens, 0);
    const totalOutputTokens = entries.reduce((s, e) => s + e.totalOutputTokens, 0);
    const durEntries = entries.filter((e) => e.avgDuration !== null);
    const avgDuration =
      durEntries.length > 0
        ? durEntries.reduce((s, e) => s + (e.avgDuration ?? 0), 0) / durEntries.length
        : null;
    const score =
      entries.length > 0
        ? Math.round(entries.reduce((s, e) => s + e.score, 0) / entries.length)
        : 0;
    const verdictPass = entries.reduce((s, e) => s + e.verdictPass, 0);
    const verdictFail = entries.reduce((s, e) => s + e.verdictFail, 0);
    const verdictFlaky = entries.reduce((s, e) => s + e.verdictFlaky, 0);

    return {
      score,
      casesWithResults: totalCases,
      totalCases,
      passingCases,
      totalRuns,
      passingRuns,
      avgDuration,
      totalTokens: totalInputTokens + totalOutputTokens,
      totalInputTokens,
      totalOutputTokens,
      verdictCounts: {
        pass: verdictPass,
        fail: verdictFail,
        flaky: verdictFlaky
      }
    };
  }, [testFiles, selectedProjectRun, historicalFileStats]);

  // Use live data while running, historical data otherwise
  const projectHealth = anyRunning ? liveHealth : (historicalHealth ?? liveHealth);

  const suiteProgress = useMemo(() => {
    if (!testFiles) return null;
    let total = 0,
      completed = 0;
    for (const file of testFiles) {
      const pathb64 = encodeBase64(file.path);
      for (let i = 0; i < file.case_count; i++) {
        total++;
        const cs = store.caseMap.get(createCaseKey(projectId, branchName, pathb64, i));
        if (cs?.result || cs?.error) completed++;
      }
    }
    if (total === 0) return null;
    return { total, completed, percent: Math.round((completed / total) * 100) };
  }, [testFiles, projectId, branchName, store.caseMap]);

  // Brief "Run complete" banner state
  const [justFinished, setJustFinished] = useState(false);

  // Invalidate queries when a run finishes
  const prevAnyRunning = useRef(anyRunning);
  useEffect(() => {
    if (prevAnyRunning.current && !anyRunning) {
      setJustFinished(true);
      queryClient.invalidateQueries({
        queryKey: queryKeys.testProjectRun.list(projectId)
      });
      testFiles?.forEach((f) => {
        queryClient.invalidateQueries({
          queryKey: queryKeys.testRun.list(projectId, encodeBase64(f.path))
        });
      });
      const timer = setTimeout(() => setJustFinished(false), 3000);
      return () => clearTimeout(timer);
    }
    prevAnyRunning.current = anyRunning;
  }, [anyRunning, queryClient, projectId, testFiles]);

  // Trend chart — index of the currently selected run (for highlight)
  const trendPoints = useMemo((): ProjectRunTrendPoint[] => {
    if (!projectRuns || !testFiles) return [];
    return projectRuns
      .filter((pr) => pr.score !== null || pr.file_scores.length > 0)
      .slice()
      .reverse()
      .map((pr) => {
        const label = formatRunLabel(pr);
        const d = new Date(pr.created_at);
        const shortLabel = d.toLocaleDateString(undefined, {
          month: "short",
          day: "numeric",
          year: "numeric"
        });
        const isFailed = pr.score === null && pr.total_cases === null && pr.file_scores.length > 0;
        const fileBreakdown = pr.file_scores
          .filter((f) => f.score !== null)
          .map((f) => {
            const file = testFiles.find((tf) => tf.path === f.source_id);
            const name =
              file?.name ??
              f.source_id
                .split("/")
                .pop()
                ?.replace(/\.test\.(yml|yaml)$/, "") ??
              f.source_id;
            return { name, score: f.score! };
          });
        return {
          label,
          shortLabel,
          score: pr.score ?? 0,
          failed: isFailed,
          runId: pr.id,
          fileBreakdown
        };
      });
  }, [projectRuns, testFiles]) as (ProjectRunTrendPoint & { runId: string })[];

  const selectedTrendIdx = useMemo(() => {
    if (!selectedProjectRunId) return undefined;
    const idx = trendPoints.findIndex(
      (p) => (p as ProjectRunTrendPoint & { runId: string }).runId === selectedProjectRunId
    );
    return idx >= 0 ? idx : undefined;
  }, [trendPoints, selectedProjectRunId]);

  return (
    <div className='flex h-full flex-col'>
      <PageHeader
        icon={ShieldCheck}
        title='Tests'
        actions={
          <div className='flex items-center gap-2'>
            {/* Project-wide run selector */}
            {projectRuns && projectRuns.length > 0 && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant={isViewingLatest ? "outline" : "secondary"}
                    size='sm'
                    className='gap-1.5 text-xs'
                  >
                    <History className='h-3.5 w-3.5' />
                    {selectedProjectRun ? formatRunLabel(selectedProjectRun) : "Select run"}
                    <ChevronDown className='h-3 w-3 opacity-50' />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align='end' className='w-80'>
                  <DropdownMenuLabel className='text-xs'>Suite Runs</DropdownMenuLabel>
                  <DropdownMenuSeparator />
                  {projectRuns.map((pr, i) => (
                    <DropdownMenuItem
                      key={pr.id}
                      className='flex items-center gap-1 text-xs'
                      onSelect={() => handleSelectRun(pr.id)}
                    >
                      <span
                        className={cn(
                          "flex-1 truncate",
                          selectedProjectRunId === pr.id && "font-semibold"
                        )}
                      >
                        {formatRunLabel(pr)}
                        {i === 0 && (
                          <span className='ml-1.5 text-[10px] text-muted-foreground'>latest</span>
                        )}
                      </span>
                      {pr.score !== null ? (
                        <Badge
                          variant='outline'
                          className={`ml-1 shrink-0 text-[10px] ${scoreClass(Math.round(pr.score * 100))}`}
                        >
                          {Math.round(pr.score * 100)}%
                        </Badge>
                      ) : pr.total_cases === null && pr.file_scores.length > 0 ? (
                        <Badge
                          variant='outline'
                          className='ml-1 shrink-0 gap-1 border-destructive/50 text-[10px] text-destructive'
                        >
                          <AlertCircle className='h-3 w-3 text-destructive' />
                          Failed
                        </Badge>
                      ) : null}
                      <Button
                        variant='ghost'
                        size='icon'
                        className='ml-1 h-5 w-5 shrink-0 opacity-60 hover:opacity-100'
                        onClick={(e) => {
                          e.stopPropagation();
                          e.preventDefault();
                          deleteProjectRun.mutate({ projectRunId: pr.id });
                          if (selectedProjectRunId === pr.id) handleSelectRun(latestProjectRunId);
                        }}
                      >
                        <svg
                          className='h-3 w-3'
                          fill='none'
                          viewBox='0 0 24 24'
                          stroke='currentColor'
                          strokeWidth={2}
                        >
                          <path
                            strokeLinecap='round'
                            strokeLinejoin='round'
                            d='M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16'
                          />
                        </svg>
                      </Button>
                    </DropdownMenuItem>
                  ))}
                  <DropdownMenuSeparator />
                  <DropdownMenuItem asChild className='text-muted-foreground text-xs'>
                    <Link to={ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.TESTS.RUNS}>
                      View all runs →
                    </Link>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}

            <Button
              variant='outline'
              size='sm'
              className='gap-1'
              onClick={createTestFile.openDialog}
            >
              <Plus className='h-3 w-3' />
              New Test File
            </Button>
            {!anyRunning && (
              <Button
                variant='default'
                size='sm'
                className='gap-1'
                onClick={() => openRunDialog(null)}
              >
                <Play className='h-3 w-3' />
                Run All
              </Button>
            )}
            {anyRunning && (
              <Button
                variant='destructive'
                size='sm'
                className='gap-1'
                onClick={() => store.stopAll()}
              >
                <Square className='h-3 w-3 fill-current' />
                Stop
              </Button>
            )}
          </div>
        }
      />

      {/* Run context indicator */}
      {!anyRunning && selectedProjectRun && !isViewingLatest && (
        <div className='flex items-center gap-2 border-b bg-muted/30 px-4 py-1.5 text-muted-foreground text-xs'>
          <History className='h-3.5 w-3.5 shrink-0' />
          <span>
            Viewing historical run:{" "}
            <span className='font-medium text-foreground'>
              {formatRunLabel(selectedProjectRun)}
            </span>
          </span>
          <Button
            variant='ghost'
            size='sm'
            className='ml-auto h-5 px-2 text-[11px]'
            onClick={() => latestProjectRunId && handleSelectRun(latestProjectRunId)}
          >
            Back to latest
          </Button>
        </div>
      )}
      {!anyRunning && selectedProjectRun && isViewingLatest && !justFinished && (
        <div className='flex items-center gap-2 border-b px-4 py-1 text-[11px] text-muted-foreground/70'>
          <span>
            Viewing:{" "}
            <span className='font-medium text-muted-foreground'>
              {formatRunLabel(selectedProjectRun)}
            </span>
          </span>
        </div>
      )}

      <div className='scrollbar-gutter-auto min-h-0 flex-1 overflow-auto p-4'>
        {/* Running test suite — above metrics so it's clear a run is in progress */}
        {anyRunning && suiteProgress && (
          <div className='mb-3 rounded-lg border bg-primary/5 px-3 py-2'>
            <div className='mb-1.5 flex items-center gap-2'>
              <Spinner className='size-3 text-primary' />
              <div className='flex flex-col'>
                <span className='font-medium text-sm'>Running test suite</span>
                <span className='text-[11px] text-muted-foreground'>
                  Live run in progress — metrics updating
                </span>
              </div>
              <span className='ml-auto text-muted-foreground text-sm tabular-nums'>
                {suiteProgress.completed}/{suiteProgress.total} cases
              </span>
              <span className='font-medium text-sm tabular-nums'>{suiteProgress.percent}%</span>
            </div>
            <div className='h-1.5 overflow-hidden rounded-full bg-muted'>
              <div
                className='h-full rounded-full bg-primary transition-all duration-500'
                style={{ width: `${suiteProgress.percent}%` }}
              />
            </div>
          </div>
        )}

        {/* Brief completion banner */}
        {justFinished && !anyRunning && (
          <div className='mb-3 rounded-lg border border-success/30 bg-success/5 px-3 py-2 transition-opacity duration-500'>
            <div className='flex items-center gap-2'>
              <CheckCircle2 className='h-3.5 w-3.5 text-success' />
              <span className='font-medium text-sm text-success'>Run complete</span>
              <span className='ml-auto text-[11px] text-muted-foreground'>Results updated</span>
            </div>
          </div>
        )}

        {/* Failed run banner */}
        {!anyRunning &&
          !justFinished &&
          selectedProjectRun &&
          selectedProjectRun.score === null &&
          selectedProjectRun.total_cases === null &&
          selectedProjectRun.file_scores.length > 0 && (
            <div className='mb-3 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2'>
              <div className='flex items-center gap-2'>
                <AlertCircle className='h-3.5 w-3.5 text-destructive' />
                <span className='font-medium text-destructive text-sm'>Run failed</span>
                <span className='ml-auto text-[11px] text-muted-foreground'>
                  No results were recorded — something went wrong
                </span>
              </div>
            </div>
          )}

        {/* Invisible stats loaders for selected historical run */}
        {selectedProjectRun &&
          !anyRunning &&
          testFiles?.map((file) => {
            const pb64 = encodeBase64(file.path);
            const runIndex = effectiveRunIndexMap.get(pb64) ?? null;
            return runIndex !== null ? (
              <FileStatsLoader
                key={`${selectedProjectRun.id}:${pb64}`}
                pathb64={pb64}
                runIndex={runIndex}
                onData={(data) => reportFileStats(pb64, data)}
              />
            ) : null;
          })}

        {/* Project health summary */}
        {testFiles &&
          testFiles.length > 0 &&
          (() => {
            const totalCases = testFiles.reduce((s, f) => s + f.case_count, 0);
            const hasResults = projectHealth !== null && projectHealth.casesWithResults > 0;
            const score = hasResults ? projectHealth.score : 0;
            const passingCases = hasResults ? projectHealth.passingCases : 0;
            const shownTotal = hasResults ? projectHealth.totalCases : totalCases;
            const casesCompleted = hasResults ? projectHealth.casesWithResults : 0;
            const isPartial = anyRunning && hasResults && casesCompleted < totalCases;
            const totalRuns = hasResults ? projectHealth.totalRuns : 0;
            const passingRuns = hasResults ? projectHealth.passingRuns : 0;
            const consistency = totalRuns > 0 ? Math.round((passingRuns / totalRuns) * 100) : 0;
            const avgDuration = hasResults ? projectHealth.avgDuration : null;
            const totalTokens = hasResults ? projectHealth.totalTokens : 0;
            const inputTokens = hasResults ? projectHealth.totalInputTokens : 0;
            const outputTokens = hasResults ? projectHealth.totalOutputTokens : 0;
            const verdicts = hasResults
              ? projectHealth.verdictCounts
              : { pass: 0, fail: 0, flaky: 0 };

            const partialSuffix = isPartial ? ` (${casesCompleted}/${totalCases} completed)` : "";

            return (
              <div className='mb-3 grid grid-cols-2 gap-2 lg:grid-cols-5'>
                <DashboardStatsCard
                  title={isPartial ? "Score (live)" : "Score"}
                  value={`${score}%`}
                  subtitle={`${passingCases}/${shownTotal} cases passing${partialSuffix}`}
                  icon={<CheckCircle2 className='h-4 w-4' />}
                  variant={hasResults ? getScoreVariant(score) : "default"}
                />
                <DashboardStatsCard
                  title={isPartial ? "Consistency (live)" : "Consistency"}
                  value={`${consistency}%`}
                  subtitle={
                    totalRuns > 0
                      ? `${passingRuns}/${totalRuns} runs passing${partialSuffix}`
                      : "No runs yet"
                  }
                  icon={<Layers className='h-4 w-4' />}
                  variant={
                    !hasResults
                      ? "default"
                      : consistency === 100
                        ? "success"
                        : consistency >= 80
                          ? "warning"
                          : "danger"
                  }
                />
                <DashboardStatsCard
                  title='Avg Latency'
                  value={avgDuration !== null ? formatDuration(avgDuration) : "\u2014"}
                  subtitle={isPartial ? `Per run average${partialSuffix}` : "Per run average"}
                  icon={<Clock className='h-4 w-4' />}
                />
                <DashboardStatsCard
                  title='Total Tokens'
                  value={totalTokens > 0 ? formatTokens(totalTokens) : "\u2014"}
                  subtitle={
                    totalTokens > 0
                      ? `${formatTokens(inputTokens)} in / ${formatTokens(outputTokens)} out`
                      : "No usage yet"
                  }
                  icon={<Zap className='h-4 w-4' />}
                />
                <Card className='flex items-center justify-center overflow-hidden'>
                  <CardContent className='p-3'>
                    <VerdictDonutChart
                      pass={verdicts.pass}
                      fail={verdicts.fail}
                      flaky={verdicts.flaky}
                    />
                  </CardContent>
                </Card>
              </div>
            );
          })()}

        {/* Trend chart */}
        {trendPoints.length >= 2 && (
          <Card className='mb-3'>
            <CardHeader className='px-3 pt-3 pb-0'>
              <div className='flex items-center justify-between'>
                <CardTitle className='font-medium text-xs'>Pass Rate History</CardTitle>
                <span className='text-[10px] text-muted-foreground/60'>
                  Click a bar to view that run
                </span>
              </div>
            </CardHeader>
            <CardContent className='px-3 pt-1 pb-3'>
              <TrendBarChart
                points={trendPoints}
                selectedIdx={selectedTrendIdx}
                onBarClick={(idx) => {
                  const run = (trendPoints as (ProjectRunTrendPoint & { runId: string })[])[idx];
                  if (run) handleSelectRun(run.runId);
                }}
              />
            </CardContent>
          </Card>
        )}

        {/* Loading state */}
        {isLoading && (
          <div className='space-y-4'>
            {Array.from({ length: 3 }).map((_, i) => (
              <Skeleton key={i} className='h-24 w-full' />
            ))}
          </div>
        )}

        {/* Empty state */}
        {!isLoading && testFiles?.length === 0 && (
          <div className='flex flex-col items-center justify-center rounded-lg border border-dashed p-12 text-center'>
            <ShieldCheck className='mb-3 h-10 w-10 text-muted-foreground' />
            <p className='font-medium text-sm'>No test files found</p>
            <p className='mt-1 text-muted-foreground text-xs'>
              Create a <code>.test.yml</code> file to get started with testing.
            </p>
          </div>
        )}

        {/* Test file cards */}
        <div className='space-y-3'>
          {testFiles?.map((file) => {
            const pathb64 = encodeBase64(file.path);
            const derivedName = (file.path.split("/").pop() ?? file.path).replace(
              /\.test\.(yml|yaml)$/,
              ""
            );
            const displayName = file.name ?? derivedName;
            const selectedRunIndex = effectiveRunIndexMap.get(pathb64) ?? null;
            return (
              <TestFileCard
                key={file.path}
                displayName={displayName}
                target={file.target}
                caseCount={file.case_count}
                pathb64={pathb64}
                projectId={projectId}
                selectedRunIndex={selectedRunIndex}
                onRunFile={() => openRunDialog(pathb64)}
                onStopFile={() => store.stopFile(projectId, branchName, pathb64)}
                onNavigateToDetail={(caseIndex, runIndex) => {
                  const url = ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.TESTS.TEST_FILE(pathb64);
                  const params = new URLSearchParams();
                  if (runIndex !== undefined) params.set("run_index", String(runIndex));
                  navigate(
                    `${url}${params.toString() ? `?${params.toString()}` : ""}${caseIndex !== undefined ? `#case-${caseIndex}` : ""}`
                  );
                }}
              />
            );
          })}
        </div>
      </div>

      {/* Run naming dialog */}
      <Dialog open={runNameDialogOpen} onOpenChange={setRunNameDialogOpen}>
        <DialogContent className='sm:max-w-sm'>
          <DialogHeader>
            <DialogTitle className='flex items-center gap-2'>
              <Play className='h-4 w-4' />
              Name this run
            </DialogTitle>
          </DialogHeader>
          <div className='grid gap-4 py-2'>
            <div className='grid gap-2'>
              <Label htmlFor='runName'>Label (optional)</Label>
              <Input
                id='runName'
                value={pendingRunName}
                onChange={(e) => setPendingRunName(e.target.value)}
                onKeyDown={(e) => {
                  // Cannot reject: `handleConfirmRun` catches both mutations it awaits and
                  // runs the cases either way.
                  if (e.key === "Enter") void handleConfirmRun();
                }}
                placeholder={`Run — ${new Date().toLocaleDateString()}`}
                autoFocus
              />
              <p className='text-muted-foreground text-xs'>
                Leave blank to use the current date/time as the label.
              </p>
            </div>
          </div>
          <DialogFooter>
            <Button variant='outline' onClick={() => setRunNameDialogOpen(false)}>
              Cancel
            </Button>
            <Button onClick={handleConfirmRun}>
              <Play className='mr-1.5 h-3 w-3' />
              Start
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* New test file dialog */}
      <Dialog open={createTestFile.dialogOpen} onOpenChange={createTestFile.setDialogOpen}>
        <DialogContent className='sm:max-w-md'>
          <DialogHeader>
            <DialogTitle className='flex items-center gap-2'>
              <FlaskConical className='h-5 w-5' />
              New Test File
            </DialogTitle>
          </DialogHeader>
          <div className='grid gap-4 py-4'>
            <div className='grid gap-2'>
              <Label htmlFor='testFileName'>Name</Label>
              <div className='flex items-center gap-2'>
                <Input
                  id='testFileName'
                  ref={createTestFile.inputRef}
                  value={createTestFile.fileName}
                  onChange={(e) => {
                    createTestFile.setFileName(e.target.value);
                    createTestFile.setError(null);
                  }}
                  onKeyDown={createTestFile.handleKeyDown}
                  placeholder='my-agent'
                  className={createTestFile.error ? "border-destructive" : ""}
                />
                <span className='whitespace-nowrap text-muted-foreground text-sm'>.test.yml</span>
              </div>
              {createTestFile.error && <FieldError>{createTestFile.error}</FieldError>}
            </div>
          </div>
          <DialogFooter>
            <Button
              variant='outline'
              onClick={() => createTestFile.setDialogOpen(false)}
              disabled={createTestFile.isCreating}
            >
              Cancel
            </Button>
            <Button
              onClick={createTestFile.handleCreate}
              disabled={createTestFile.isCreating || !createTestFile.fileName.trim()}
            >
              {createTestFile.isCreating ? <Spinner /> : "Create"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

// --- Invisible file stats loader ---

const FileStatsLoader: React.FC<{
  pathb64: string;
  runIndex: number;
  onData: (data: HistoricalFileStatsEntry | null) => void;
}> = ({ pathb64, runIndex, onData }) => {
  const { project } = useCurrentProjectBranch();
  const { data: run } = useTestRunDetail(pathb64, runIndex, true);
  const onDataRef = useRef(onData);
  onDataRef.current = onData;

  useEffect(() => {
    if (!run || run.cases.length === 0) {
      onDataRef.current(null);
      return;
    }
    const cases = run.cases;
    const scored = cases.length;
    const totalScore = cases.reduce((s, c) => s + getEffectiveScore(c) * 100, 0);
    const passing = cases.filter((c) => getHistoricalVerdict(c) === "pass").length;
    const failing = cases.filter((c) => getHistoricalVerdict(c) === "fail").length;
    const flaky = cases.filter((c) => getHistoricalVerdict(c) === "flaky").length;
    const durCases = cases.filter((c) => c.avg_duration_ms !== null);
    const avgDuration =
      durCases.length > 0
        ? durCases.reduce((s, c) => s + (c.avg_duration_ms ?? 0), 0) / durCases.length
        : null;
    const totalInputTokens = cases.reduce((s, c) => s + (c.input_tokens ?? 0), 0);
    const totalOutputTokens = cases.reduce((s, c) => s + (c.output_tokens ?? 0), 0);
    const totalRuns = cases.reduce((s, c) => s + c.total_runs, 0);
    const passingRuns = cases.reduce((s, c) => s + c.passing_runs, 0);
    onDataRef.current({
      score: Math.round(totalScore / scored),
      passing,
      scored,
      avgDuration,
      totalInputTokens,
      totalOutputTokens,
      totalRuns,
      passingRuns,
      verdictPass: passing,
      verdictFail: failing,
      verdictFlaky: flaky
    });
  }, [run]);

  // Suppress unused variable warning
  void project;
  return null;
};

// --- File card ---

interface TestFileCardProps {
  displayName: string;
  target: string | null;
  caseCount: number;
  pathb64: string;
  projectId: string;
  selectedRunIndex: number | null;
  onRunFile: () => void;
  onStopFile: () => void;
  onNavigateToDetail: (caseIndex?: number, runIndex?: number) => void;
}

const TestFileCard: React.FC<TestFileCardProps> = ({
  displayName,
  target,
  caseCount,
  pathb64,
  projectId,
  selectedRunIndex,
  onRunFile,
  onStopFile,
  onNavigateToDetail
}) => {
  const { branchName } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const store = useTestFileResults();

  const { data: historicalRun } = useTestRunDetail(
    pathb64,
    selectedRunIndex,
    selectedRunIndex !== null
  );

  const fileHealth = useMemo(() => {
    if (selectedRunIndex !== null && historicalRun) {
      const cases = historicalRun.cases;
      if (cases.length === 0) return null;
      const scored = cases.length;
      const totalScore = cases.reduce((s, c) => s + getEffectiveScore(c) * 100, 0);
      const passing = cases.filter((c) => getHistoricalVerdict(c) === "pass").length;
      const durCases = cases.filter((c) => c.avg_duration_ms !== null);
      const avgDuration =
        durCases.length > 0
          ? durCases.reduce((s, c) => s + (c.avg_duration_ms ?? 0), 0) / durCases.length
          : null;
      const inputTokens = cases.reduce((s, c) => s + (c.input_tokens ?? 0), 0);
      const outputTokens = cases.reduce((s, c) => s + (c.output_tokens ?? 0), 0);
      return {
        score: Math.round(totalScore / scored),
        passing,
        scored,
        avgDuration: avgDuration ?? null,
        totalInputTokens: inputTokens,
        totalOutputTokens: outputTokens
      };
    }

    let scored = 0,
      totalScore = 0,
      passing = 0;
    let durationSum = 0,
      durationCount = 0;
    let totalInputTokens = 0,
      totalOutputTokens = 0;
    for (let i = 0; i < caseCount; i++) {
      const cs = store.caseMap.get(createCaseKey(projectId, branchName, pathb64, i));
      if (cs?.result) {
        const s = getScorePercent(cs.result.metrics);
        if (s !== null) {
          totalScore += s;
          scored++;
          const c = getConsistency(cs.result.metrics);
          if (c.passing === c.total && c.total > 0) passing++;
        }
        for (const m of cs.result.metrics) {
          if (m.type !== MetricKind.Recall) {
            for (const r of m.records) {
              if (!isErrorRecord(r)) {
                totalInputTokens += r.input_tokens;
                totalOutputTokens += r.output_tokens;
                if (r.duration_ms > 0) {
                  durationSum += r.duration_ms;
                  durationCount++;
                }
              }
            }
          }
        }
      }
    }
    if (scored === 0) return null;
    return {
      score: Math.round(totalScore / scored),
      passing,
      scored,
      avgDuration: durationCount > 0 ? durationSum / durationCount : null,
      totalInputTokens,
      totalOutputTokens
    };
  }, [projectId, branchName, pathb64, caseCount, selectedRunIndex, historicalRun, store.caseMap]);

  const fileState = useMemo(() => {
    let running = 0,
      completed = 0;
    for (let i = 0; i < caseCount; i++) {
      const cs = store.caseMap.get(createCaseKey(projectId, branchName, pathb64, i));
      if (cs?.state === EvalEventState.Started || cs?.state === EvalEventState.Progress) running++;
      if (cs?.result || cs?.error) completed++;
    }
    return { completed, isRunning: running > 0 };
  }, [projectId, branchName, pathb64, caseCount, store.caseMap]);

  return (
    <Collapsible>
      <div className='rounded-lg border'>
        <CollapsibleTrigger className='flex w-full items-center justify-between px-4 py-3 hover:bg-muted/50'>
          <div className='flex items-center gap-3 text-left'>
            {fileState.isRunning && <Spinner className='text-primary' />}
            <div>
              <p className='font-medium text-sm'>{displayName}</p>
              {target && <p className='text-muted-foreground text-xs'>Target: {target}</p>}
            </div>
          </div>
          <div className='flex items-center gap-2'>
            {fileState.isRunning ? (
              <div className='flex items-center gap-1.5'>
                <PieProgress completed={fileState.completed} total={caseCount} size={22} />
                <span className='text-muted-foreground text-xs tabular-nums'>
                  {fileState.completed}/{caseCount}
                </span>
              </div>
            ) : fileHealth ? (
              <>
                <Badge variant='outline' className={`text-xs ${scoreClass(fileHealth.score)}`}>
                  {fileHealth.score}%
                </Badge>
                <span className='text-muted-foreground text-xs'>
                  {fileHealth.passing}/{fileHealth.scored}
                </span>
              </>
            ) : (
              <Badge variant='secondary' className='text-xs'>
                {caseCount} {caseCount === 1 ? "case" : "cases"}
              </Badge>
            )}
            <Button
              variant='ghost'
              size='icon'
              className={
                fileState.isRunning ? "h-7 w-7 text-destructive hover:text-destructive" : "h-7 w-7"
              }
              onClick={(e) => {
                e.stopPropagation();
                if (fileState.isRunning) onStopFile();
                else onRunFile();
              }}
              title={fileState.isRunning ? "Stop this file" : "Run all cases in this file"}
            >
              {fileState.isRunning ? (
                <Square className='h-3 w-3 fill-current' />
              ) : (
                <Play className='h-3 w-3' />
              )}
            </Button>
            <Link
              to={ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.FILES.FILE(pathb64)}
              onClick={(e) => e.stopPropagation()}
            >
              <Button variant='ghost' size='icon' className='h-7 w-7'>
                <Pencil className='h-3 w-3' />
              </Button>
            </Link>
          </div>
        </CollapsibleTrigger>
        {fileHealth &&
          !fileState.isRunning &&
          (fileHealth.avgDuration !== null ||
            fileHealth.totalInputTokens + fileHealth.totalOutputTokens > 0) && (
            <div className='flex items-center gap-4 border-t px-4 py-1.5 text-muted-foreground text-xs'>
              {fileHealth.avgDuration !== null && (
                <span className='flex items-center gap-1'>
                  <Clock className='h-3 w-3' />
                  {formatDuration(fileHealth.avgDuration)} avg latency
                </span>
              )}
              {fileHealth.totalInputTokens + fileHealth.totalOutputTokens > 0 && (
                <span className='flex items-center gap-1'>
                  <Zap className='h-3 w-3' />
                  {formatTokens(fileHealth.totalInputTokens + fileHealth.totalOutputTokens)} tokens
                  <span className='opacity-60'>
                    ({formatTokens(fileHealth.totalInputTokens)} in /{" "}
                    {formatTokens(fileHealth.totalOutputTokens)} out)
                  </span>
                </span>
              )}
            </div>
          )}
        <CollapsibleContent>
          <div className='border-t'>
            {selectedRunIndex !== null && historicalRun ? (
              <HistoricalCaseList
                cases={historicalRun.cases}
                runIndex={selectedRunIndex}
                onNavigate={onNavigateToDetail}
              />
            ) : (
              <TestFileCasesList
                pathb64={pathb64}
                projectId={projectId}
                caseCount={caseCount}
                onNavigate={onNavigateToDetail}
              />
            )}
          </div>
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
};

// --- Historical case list ---

const HistoricalCaseList: React.FC<{
  cases: TestRunCaseResult[];
  runIndex: number;
  onNavigate: (caseIndex: number, runIndex: number) => void;
}> = ({ cases, runIndex, onNavigate }) => (
  <div className='divide-y'>
    {cases.map((c) => {
      const verdict = getHistoricalVerdict(c);
      return (
        <button
          key={c.case_index}
          type='button'
          className='flex w-full items-center gap-3 px-4 py-2 text-left transition-colors hover:bg-muted/50'
          onClick={() => onNavigate(c.case_index, runIndex)}
        >
          <VerdictIcon verdict={verdict} />
          <span className='flex-1 truncate text-sm' title={c.prompt}>
            {c.prompt}
          </span>
          {c.avg_duration_ms !== null && (
            <span className='flex shrink-0 items-center gap-1 text-muted-foreground text-xs'>
              <Clock className='h-3 w-3' />
              {formatDuration(c.avg_duration_ms)}
            </span>
          )}
          {(c.input_tokens ?? 0) + (c.output_tokens ?? 0) > 0 && (
            <span className='flex shrink-0 items-center gap-1 text-muted-foreground text-xs'>
              <Zap className='h-3 w-3' />
              {formatTokens((c.input_tokens ?? 0) + (c.output_tokens ?? 0))}
            </span>
          )}
          <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
            {c.passing_runs}/{c.total_runs}
          </span>
          <span className='shrink-0 text-muted-foreground text-xs'>{verdictLabel(verdict)}</span>
        </button>
      );
    })}
  </div>
);

// --- Live case list ---

interface TestFileCasesListProps {
  pathb64: string;
  projectId: string;
  caseCount: number;
  onNavigate: (caseIndex: number, runIndex?: number) => void;
}

const TestFileCasesList: React.FC<TestFileCasesListProps> = ({
  pathb64,
  projectId,
  caseCount,
  onNavigate
}) => {
  const { branchName } = useCurrentProjectBranch();
  const { data: testFile, isLoading } = useTestFile(pathb64);
  const store = useTestFileResults();

  if (isLoading) {
    return (
      <div className='space-y-1 p-3'>
        {Array.from({ length: caseCount }).map((_, i) => (
          <Skeleton key={i} className='h-7 w-full' />
        ))}
      </div>
    );
  }

  const cases: TestFileConfig["cases"] = testFile?.cases ?? [];

  return (
    <div className='divide-y'>
      {cases.map((testCase, index) => {
        const caseState = store.getCase(projectId, branchName, pathb64, index);
        const verdict = getCaseVerdict(caseState);
        const consistency = caseState.result ? getConsistency(caseState.result.metrics) : null;
        const metrics = caseState.result ? getCaseMetrics(caseState.result) : null;

        return (
          <button
            key={index}
            type='button'
            className='flex w-full items-center gap-3 px-4 py-2 text-left transition-colors hover:bg-muted/50'
            onClick={() => onNavigate(index)}
          >
            <VerdictIcon verdict={verdict} />
            <span className='flex-1 truncate text-sm' title={testCase.prompt}>
              {testCase.prompt}
            </span>
            {metrics && metrics.avgDuration !== null && (
              <span className='flex shrink-0 items-center gap-1 text-muted-foreground text-xs'>
                <Clock className='h-3 w-3' />
                {formatDuration(metrics.avgDuration)}
              </span>
            )}
            {metrics && metrics.totalInputTokens + metrics.totalOutputTokens > 0 && (
              <span className='flex shrink-0 items-center gap-1 text-muted-foreground text-xs'>
                <Zap className='h-3 w-3' />
                {formatTokens(metrics.totalInputTokens + metrics.totalOutputTokens)}
              </span>
            )}
            {consistency && (
              <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
                {consistency.passing}/{consistency.total}
              </span>
            )}
            <span className='shrink-0 text-muted-foreground text-xs'>{verdictLabel(verdict)}</span>
          </button>
        );
      })}
    </div>
  );
};

export default TestsDashboardPage;
