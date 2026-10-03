import {
  AlignLeft,
  BadgeCheck,
  BarChart2,
  BookOpen,
  Brain,
  Bug,
  Check,
  ChevronRight,
  Database,
  FilePen,
  FileText,
  FlaskConical,
  FolderPlus,
  FolderSearch,
  GitBranch,
  GitMerge,
  Hammer,
  Info,
  Layers,
  Loader2,
  MessageSquare,
  Network,
  ScanSearch,
  Search,
  ShieldCheck,
  Sprout,
  Table2,
  TextSearch,
  Trash2,
  Wrench,
  X,
  Zap
} from "lucide-react";
import { useMemo, useState } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type {
  AnalyticsStep,
  ArtifactItem,
  AutomationItem,
  SelectableItem,
  SqlItem,
  StepLlmUsage,
  TextItem,
  ThinkingItem,
  TraceItem
} from "@/hooks/analyticsSteps";
import { cn } from "@/libs/shadcn/utils";
import { VERIFIED_SQL_FILE_TOOLTIP, VERIFIED_TOOLTIP } from "../constants";
import AutomationDelegationCard from "./AutomationDelegationCard";
import BuilderDelegationCard from "./BuilderDelegationCard";

// ── LLM Usage Tooltip ────────────────────────────────────────────────────────

function formatTokens(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
}

function formatMs(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
}

function LlmUsageTooltip({ usage }: { usage: StepLlmUsage }) {
  const llmInferenceMs = Math.max(0, usage.durationMs - usage.toolDurationMs);
  return (
    <div className='flex flex-col gap-1 font-mono text-[10px] leading-relaxed'>
      {usage.model && (
        <div className='flex justify-between gap-4'>
          <span className='text-muted-foreground'>Model</span>
          <span>{usage.model}</span>
        </div>
      )}
      <div className='flex justify-between gap-4'>
        <span className='text-muted-foreground'>Input tokens</span>
        <span>{formatTokens(usage.inputTokens)}</span>
      </div>
      <div className='flex justify-between gap-4'>
        <span className='text-muted-foreground'>Output tokens</span>
        <span>{formatTokens(usage.outputTokens)}</span>
      </div>
      <div className='my-0.5 border-border/50 border-t' />
      <div className='flex justify-between gap-4'>
        <span className='text-muted-foreground'>Total time</span>
        <span>{formatMs(usage.durationMs)}</span>
      </div>
      <div className='flex justify-between gap-4'>
        <span className='text-muted-foreground'>↳ LLM inference</span>
        <span>{formatMs(llmInferenceMs)}</span>
      </div>
      <div className='flex justify-between gap-4'>
        <span className='text-muted-foreground'>↳ Tool execution</span>
        <span>{formatMs(usage.toolDurationMs)}</span>
      </div>
    </div>
  );
}

// ── Colors per step label ─────────────────────────────────────────────────────

type Colors = { dot: string; icon: string; border: string; bg: string };

function stepColors(label: string): Colors {
  switch (label) {
    case "Analyzing":
    case "Answering":
      return {
        dot: "bg-node-agent",
        icon: "text-node-agent",
        border: "border-node-agent/30",
        bg: "bg-node-agent/8"
      };
    case "Planning":
      return {
        dot: "bg-node-plan",
        icon: "text-node-plan",
        border: "border-node-plan/30",
        bg: "bg-node-plan/8"
      };
    case "Running":
      return {
        dot: "bg-node-query",
        icon: "text-node-query",
        border: "border-node-query/30",
        bg: "bg-node-query/8"
      };
    default:
      return {
        dot: "bg-muted-foreground",
        icon: "text-muted-foreground",
        border: "border-muted-foreground/30",
        bg: "bg-muted/8"
      };
  }
}

// ── Artifact pill ────────────────────────────────────────────────────────────

const PILL_CLASS =
  "flex shrink-0 items-center gap-1 rounded bg-muted-foreground/10 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground transition-colors hover:bg-muted-foreground/20 hover:text-foreground max-w-[120px]";

const VERIFIED_PILL_CLASS =
  "flex shrink-0 items-center gap-1 rounded bg-muted-foreground/10 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground transition-colors hover:bg-muted-foreground/20 hover:text-foreground max-w-[140px]";

interface ArtifactPillProps {
  icon: React.ElementType;
  label: string;
  verified?: boolean;
  verifiedTooltip?: string;
  isPreagg?: boolean;
  onClick: () => void;
  variant?: "default" | "builder";
  /** Whether the pill's panel is open; undefined when the host tracks no selection. */
  pressed?: boolean;
}

const ArtifactPill = ({
  icon: Icon,
  label,
  verified,
  verifiedTooltip = VERIFIED_TOOLTIP,
  isPreagg,
  onClick,
  variant = "default",
  pressed
}: ArtifactPillProps) => {
  const pill = (
    <button
      type='button'
      data-testid={`reasoning-pill-${label
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-|-$/g, "")}`}
      aria-pressed={pressed}
      onClick={onClick}
      className={cn(
        verified ? VERIFIED_PILL_CLASS : PILL_CLASS,
        variant === "builder" && "text-oxy-blue-600 hover:text-oxy-blue-700",
        pressed && "bg-primary/10 text-primary hover:text-primary"
      )}
    >
      <Icon className='h-3 w-3 shrink-0' />
      <span className='truncate'>{label}</span>
      {verified && (
        <BadgeCheck className='h-3.5 w-3.5 shrink-0 text-oxy-blue-600 dark:text-oxy-blue-500' />
      )}
      {isPreagg && <Zap className='h-3 w-3 shrink-0 text-primary' />}
    </button>
  );
  if (!verified && !isPreagg) return pill;
  if (verified && !isPreagg) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>{pill}</TooltipTrigger>
        <TooltipContent side='top'>{verifiedTooltip}</TooltipContent>
      </Tooltip>
    );
  }
  return (
    <Tooltip>
      <TooltipTrigger asChild>{pill}</TooltipTrigger>
      <TooltipContent side='top'>Served from pre-aggregation cache</TooltipContent>
    </Tooltip>
  );
};

// ── Child item renderers ──────────────────────────────────────────────────────

const ThinkingChild = ({ item, border }: { item: ThinkingItem; border: string }) => {
  const [expanded, setExpanded] = useState(false);
  const preview = item.text
    ? item.text.slice(0, 60) + (item.text.length > 60 ? "…" : "")
    : "Thinking…";

  return (
    <div>
      <button
        type='button'
        onClick={() => setExpanded((v) => !v)}
        className='flex w-full items-center gap-1.5 py-0.5 text-left'
      >
        <Brain className='h-3 w-3 shrink-0 text-muted-foreground' />
        <span className='flex-1 truncate text-muted-foreground text-xs'>{preview}</span>
        {item.isStreaming && <Loader2 className='h-3 w-3 shrink-0 animate-spin text-primary' />}
        {!item.isStreaming && <Check className='h-3 w-3 shrink-0 text-primary/60' />}
        <ChevronRight
          className={cn(
            "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
            expanded && "rotate-90"
          )}
        />
      </button>
      <div
        className={cn(
          "overflow-hidden transition-all duration-200",
          expanded ? "opacity-100" : "max-h-0 opacity-0"
        )}
      >
        {item.text && (
          <div className={cn("mt-0.5 mb-1 ml-4 overflow-y-auto border-l-2 pl-2", border)}>
            <p className='whitespace-pre-wrap text-muted-foreground text-xs leading-relaxed'>
              {item.text}
            </p>
          </div>
        )}
      </div>
    </div>
  );
};

// ── ArtifactChild helpers ─────────────────────────────────────────────────────

type JsonObject = Record<string, unknown>;

function tryParseJson(raw: string): JsonObject | null {
  try {
    let v = JSON.parse(raw);
    // toolInput is JSON.stringify'd in analyticsSteps, so the payload may be
    // a JSON-encoded string that needs a second parse.
    if (typeof v === "string") v = JSON.parse(v);
    if (v !== null && typeof v === "object" && !Array.isArray(v)) return v as JsonObject;
  } catch {}
  return null;
}

function trunc(s: string, max = 60): string {
  return s.length > max ? `${s.slice(0, max)}…` : s;
}

type ToolDisplay = {
  Icon: React.ElementType;
  label: string;
  preview: string;
  isError?: boolean;
};

function getToolDisplay(item: ArtifactItem): ToolDisplay {
  const input = tryParseJson(item.toolInput);

  switch (item.toolName) {
    // ── Ask user ──────────────────────────────────────────────────────────────
    case "ask_user": {
      const prompt = typeof input?.prompt === "string" ? input.prompt : "Asking user…";
      const output = tryParseJson(item.toolOutput ?? "");
      const answer = typeof output?.answer === "string" ? output.answer : null;
      const preview = answer ? `${trunc(prompt, 30)} → ${trunc(answer, 30)}` : trunc(prompt);
      return { Icon: MessageSquare, label: "Ask User", preview };
    }

    // ── Clarifying ────────────────────────────────────────────────────────────
    case "search_catalog": {
      const queries = Array.isArray(input?.queries) ? (input.queries as string[]) : [];
      const preview = queries.length ? queries.join(", ") : "Search catalog";
      return { Icon: Search, label: "Search Catalog", preview: trunc(preview) };
    }
    case "get_metric_definition": {
      const metric = typeof input?.metric === "string" ? input.metric : "—";
      return { Icon: Layers, label: "Metric Definition", preview: metric };
    }
    // Match the legacy tool name too so runs persisted before the rename render.
    case "search_automations":
    case "search_procedures": {
      const query = typeof input?.query === "string" ? input.query : "Search automations";
      return { Icon: Search, label: "Search Automations", preview: trunc(query) };
    }

    // ── Specifying ────────────────────────────────────────────────────────────
    case "get_join_path": {
      const from = typeof input?.from_entity === "string" ? input.from_entity : "?";
      const to = typeof input?.to_entity === "string" ? input.to_entity : "?";
      return { Icon: GitMerge, label: "Join Path", preview: `${from} → ${to}` };
    }
    case "sample_columns": {
      const cols = Array.isArray(input?.columns) ? input.columns : [];
      const preview = cols
        .map((c: Record<string, unknown>) => {
          const t = typeof c?.table === "string" ? c.table : "?";
          const col = typeof c?.column === "string" ? c.column : "?";
          return `${t}.${col}`;
        })
        .join(", ");
      return { Icon: Table2, label: "Sample Columns", preview: trunc(preview || "?") };
    }

    // ── Clarifying (semantic shortcut) ───────────────────────────────────────
    case "propose_semantic_query": {
      const measures = Array.isArray(input?.measures) ? (input.measures as string[]) : [];
      const dims = Array.isArray(input?.dimensions) ? (input.dimensions as string[]) : [];
      const preview = [...measures, ...dims].slice(0, 3).join(", ") || "Semantic query";
      return { Icon: GitMerge, label: "Semantic Query", preview: trunc(preview) };
    }

    // ── Specifying (semantic compile) ─────────────────────────────────────────
    case "compile_semantic_query": {
      const output = tryParseJson(item.toolOutput ?? "");
      const success = output?.success !== false;
      const preview = success
        ? "Compiled"
        : typeof output?.error === "string"
          ? output.error
          : "Compile failed";
      return { Icon: GitBranch, label: "Compile Semantic Query", preview: trunc(preview) };
    }

    // ── Solving ───────────────────────────────────────────────────────────────
    case "execute_preview": {
      const sql = typeof input?.sql === "string" ? (input.sql.trim().split("\n")[0] ?? "") : "";
      return { Icon: Database, label: "Preview Query", preview: trunc(sql) };
    }

    // ── Interpreting ──────────────────────────────────────────────────────────
    case "render_chart": {
      const chartType = typeof input?.chart_type === "string" ? input.chart_type : "chart";
      const title = typeof input?.title === "string" ? input.title : null;
      const preview = title ? `${title} · ${chartType}` : chartType;
      return { Icon: BarChart2, label: "Render Chart", preview: trunc(preview) };
    }

    // ── Builder tools ──────────────────────────────────────────────────────────
    case "file_change": {
      const filePath = typeof input?.file_path === "string" ? input.file_path : "?";
      const isDelete = input?.delete === true;
      const hasOldContent = typeof input?.old_content === "string";
      const action = isDelete ? "Delete" : hasOldContent ? "Update" : "Create";
      const output = tryParseJson(item.toolOutput ?? "");
      const status =
        typeof output?.status === "string" && output.status !== "awaiting_response"
          ? output.status
          : null;
      const preview = status
        ? `${action} ${trunc(filePath, 30)} · ${status}`
        : `${action} ${trunc(filePath, 40)}`;
      return { Icon: FilePen, label: "Propose Change", preview };
    }

    case "write_file": {
      const filePath =
        typeof input?.file_path === "string"
          ? input.file_path
          : typeof input?.path === "string"
            ? input.path
            : "?";
      return { Icon: FilePen, label: "Write File", preview: trunc(filePath) };
    }

    case "edit_file": {
      const filePath =
        typeof input?.file_path === "string"
          ? input.file_path
          : typeof input?.path === "string"
            ? input.path
            : "?";
      return { Icon: FilePen, label: "Edit File", preview: trunc(filePath) };
    }

    case "delete_file": {
      const filePath =
        typeof input?.file_path === "string"
          ? input.file_path
          : typeof input?.path === "string"
            ? input.path
            : "?";
      return { Icon: Trash2, label: "Delete File", preview: trunc(filePath) };
    }

    case "read_file": {
      const filePath =
        typeof input?.file_path === "string"
          ? input.file_path
          : typeof input?.path === "string"
            ? input.path
            : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const totalLines = typeof output?.total_lines === "number" ? output.total_lines : null;
      const preview =
        totalLines !== null ? `${trunc(filePath, 35)} · ${totalLines} lines` : trunc(filePath);
      return { Icon: FileText, label: "Read File", preview };
    }

    case "search_files": {
      const pattern = typeof input?.pattern === "string" ? input.pattern : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const count =
        typeof output?.count === "number"
          ? output.count
          : Array.isArray(output?.files)
            ? output.files.length
            : null;
      const preview = count !== null ? `${trunc(pattern, 35)} · ${count} files` : trunc(pattern);
      return { Icon: FolderSearch, label: "Search Files", preview };
    }

    case "lookup_schema": {
      const objectName = typeof input?.object_name === "string" ? input.object_name : "?";
      return { Icon: BookOpen, label: "Lookup Schema", preview: trunc(objectName) };
    }

    case "run_tests": {
      const filePath = typeof input?.file_path === "string" ? input.file_path : null;
      const scope = filePath ? trunc(filePath, 35) : "All tests";
      const output = tryParseJson(item.toolOutput ?? "");
      const testsRun = typeof output?.tests_run === "number" ? output.tests_run : null;
      const preview = testsRun !== null ? `${scope} · ${testsRun} run` : scope;
      return { Icon: FlaskConical, label: "Run Tests", preview };
    }

    case "validate_project": {
      const filePath = typeof input?.file_path === "string" ? input.file_path : null;
      const scope = filePath ? trunc(filePath, 35) : "Whole project";
      const output = tryParseJson(item.toolOutput ?? "");
      const isValid = output?.valid === true;
      const errorCount =
        typeof output?.error_count === "number"
          ? output.error_count
          : Array.isArray(output?.errors)
            ? output.errors.length
            : null;
      const status =
        output !== null
          ? isValid
            ? "Valid"
            : errorCount !== null
              ? `${errorCount} errors`
              : "Errors found"
          : null;
      const preview = status ? `${scope} · ${status}` : scope;
      return { Icon: ShieldCheck, label: "Validate Project", preview };
    }

    case "search_text": {
      const pattern = typeof input?.pattern === "string" ? input.pattern : "?";
      const fileGlob = typeof input?.file_glob === "string" ? input.file_glob : null;
      const output = tryParseJson(item.toolOutput ?? "");
      const count = typeof output?.count === "number" ? output.count : null;
      const truncated = output?.truncated === true;
      const scope = fileGlob ? ` in ${fileGlob}` : "";
      const preview =
        count !== null
          ? `${trunc(pattern, 30)}${scope} · ${count}${truncated ? "+" : ""} matches`
          : `${trunc(pattern, 40)}${scope}`;
      return { Icon: TextSearch, label: "Search Text", preview };
    }

    case "execute_sql": {
      const db = typeof input?.database === "string" ? input.database : null;
      const output = tryParseJson(item.toolOutput ?? "");
      const rowCount = typeof output?.row_count === "number" ? output.row_count : null;
      const parts = [db ?? "SQL", rowCount !== null ? `${rowCount} rows` : null].filter(Boolean);
      return { Icon: Database, label: "Execute SQL", preview: trunc(parts.join(" · ")) };
    }

    // ── Airform / dbt tools ───────────────────────────────────────────────────
    case "test_dbt_models": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const passed = typeof output?.passed === "number" ? output.passed : null;
      const failed = typeof output?.failed === "number" ? output.failed : null;
      const preview =
        passed !== null || failed !== null
          ? `${project} · ${passed ?? 0} pass · ${failed ?? 0} fail`
          : project;
      return { Icon: FlaskConical, label: "Test dbt Models", preview: trunc(preview) };
    }

    case "list_dbt_nodes": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const count = typeof output?.count === "number" ? output.count : null;
      const preview = count !== null ? `${project} · ${count} nodes` : project;
      return { Icon: Layers, label: "List dbt Nodes", preview: trunc(preview) };
    }

    case "get_dbt_lineage": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const nodeCount = Array.isArray(output?.nodes) ? output.nodes.length : null;
      const preview = nodeCount !== null ? `${project} · ${nodeCount} nodes` : project;
      return { Icon: GitBranch, label: "dbt Lineage", preview: trunc(preview) };
    }

    case "compile_dbt_model": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const model = typeof input?.model === "string" ? input.model : null;
      const output = tryParseJson(item.toolOutput ?? "");
      const compiled = typeof output?.models_compiled === "number" ? output.models_compiled : null;
      const preview = model
        ? `${project} · ${model}`
        : compiled !== null
          ? `${project} · ${compiled} models`
          : project;
      return { Icon: GitBranch, label: "Compile dbt Model", preview: trunc(preview) };
    }

    case "run_dbt_models": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const selector = typeof input?.selector === "string" ? input.selector : null;
      const output = tryParseJson(item.toolOutput ?? "");
      const results = Array.isArray(output?.results) ? output.results : [];
      const errorCount = results.filter(
        (r: Record<string, unknown>) => r.status === "ERROR"
      ).length;
      const successCount = results.filter(
        (r: Record<string, unknown>) => r.status === "SUCCESS"
      ).length;
      const preview =
        results.length > 0
          ? `${project} · ${successCount} ok${errorCount > 0 ? ` · ${errorCount} err` : ""}`
          : selector
            ? `${project} · ${selector}`
            : project;
      return { Icon: Database, label: "Run dbt Models", preview: trunc(preview) };
    }

    case "analyze_dbt_project": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const modelsAnalyzed =
        typeof output?.models_analyzed === "number" ? output.models_analyzed : null;
      const preview = modelsAnalyzed !== null ? `${project} · ${modelsAnalyzed} models` : project;
      return { Icon: ScanSearch, label: "Analyze dbt Project", preview: trunc(preview) };
    }

    case "get_dbt_column_lineage": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const edgeCount = Array.isArray(output?.edges) ? output.edges.length : null;
      const preview = edgeCount !== null ? `${project} · ${edgeCount} edges` : project;
      return { Icon: Network, label: "Column Lineage", preview: trunc(preview) };
    }

    case "parse_dbt_project": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const modelCount = typeof output?.models === "number" ? output.models : null;
      const preview = modelCount !== null ? `${project} · ${modelCount} models` : project;
      return { Icon: GitBranch, label: "Parse dbt Project", preview: trunc(preview) };
    }

    case "seed_dbt_project": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const seedsLoaded = typeof output?.seeds_loaded === "number" ? output.seeds_loaded : null;
      const preview = seedsLoaded !== null ? `${project} · ${seedsLoaded} seeds` : project;
      return { Icon: Sprout, label: "Seed dbt Project", preview: trunc(preview) };
    }

    case "debug_dbt_project": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const allOk = output?.all_ok === true;
      const issues = Array.isArray(output?.issues) ? output.issues.length : null;
      const status =
        output !== null
          ? allOk
            ? "OK"
            : issues !== null
              ? `${issues} issues`
              : "Issues found"
          : null;
      const preview = status ? `${project} · ${status}` : project;
      return { Icon: Bug, label: "Debug dbt Project", preview: trunc(preview) };
    }

    case "clean_dbt_project": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const cleaned = Array.isArray(output?.cleaned) ? output.cleaned.length : null;
      const preview = cleaned !== null ? `${project} · ${cleaned} removed` : project;
      return { Icon: Trash2, label: "Clean dbt Project", preview: trunc(preview) };
    }

    case "docs_generate_dbt": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const nodes = typeof output?.nodes === "number" ? output.nodes : null;
      const preview = nodes !== null ? `${project} · ${nodes} nodes` : project;
      return { Icon: BookOpen, label: "Generate dbt Docs", preview: trunc(preview) };
    }

    case "format_dbt_sql": {
      const project = typeof input?.project === "string" ? input.project : "?";
      const isCheck = input?.check === true;
      const output = tryParseJson(item.toolOutput ?? "");
      const changed = typeof output?.files_changed === "number" ? output.files_changed : null;
      const mode = isCheck ? "check" : "format";
      const preview = changed !== null ? `${project} · ${changed} changed` : `${project} · ${mode}`;
      return { Icon: AlignLeft, label: "Format dbt SQL", preview: trunc(preview) };
    }

    case "manage_directory": {
      const operation = typeof input?.operation === "string" ? input.operation : "?";
      const path = typeof input?.path === "string" ? input.path : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const isPending = output?.status === "awaiting_response";
      const answer = typeof output?.answer === "string" ? output.answer.toLowerCase() : null;
      let preview: string;
      if (output === null || isPending) {
        preview = `${operation} · ${path}`;
      } else if (answer?.includes("accept")) {
        preview = `${operation} · ${path} · done`;
      } else if (answer?.includes("reject")) {
        preview = `${operation} · ${path} · rejected`;
      } else {
        preview = `${operation} · ${path}`;
      }
      return { Icon: FolderPlus, label: "Manage Directory", preview: trunc(preview) };
    }

    case "init_dbt_project": {
      const name = typeof input?.name === "string" ? input.name : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const isPending = output?.status === "awaiting_response";
      let preview: string;
      if (output === null || isPending) {
        preview = name;
      } else if (output.ok === true) {
        preview = `${name} · created`;
      } else {
        preview = name;
      }
      const isError = output !== null && !isPending && output.ok !== true;
      return { Icon: FolderPlus, label: "Init dbt Project", preview: trunc(preview), isError };
    }

    // ── Metric-tree tools ────────────────────────────────────────────────────
    case "explain_metric": {
      const target = typeof input?.target === "string" ? input.target : "?";
      const cur = typeof input?.current_period_end === "string" ? input.current_period_end : null;
      const prev =
        typeof input?.previous_period_end === "string" ? input.previous_period_end : null;
      const period = cur && prev ? `${prev} → ${cur}` : null;
      const preview = period ? `${target} · ${period}` : target;
      return { Icon: GitBranch, label: "Explain Metric", preview: trunc(preview) };
    }

    case "find_opportunities": {
      const target = typeof input?.target === "string" ? input.target : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const dimCount = Array.isArray(output?.dimensions) ? output.dimensions.length : null;
      const preview = dimCount !== null ? `${target} · ${dimCount} dimension(s)` : target;
      return { Icon: ScanSearch, label: "Find Opportunities", preview: trunc(preview) };
    }

    case "metric_sensitivity": {
      const target = typeof input?.target === "string" ? input.target : "?";
      const output = tryParseJson(item.toolOutput ?? "");
      const driverCount = Array.isArray(output?.drivers) ? output.drivers.length : null;
      const preview = driverCount !== null ? `${target} · ${driverCount} driver(s)` : target;
      return { Icon: Network, label: "Metric Sensitivity", preview: trunc(preview) };
    }

    case "predict_impact": {
      const changes = Array.isArray(input?.changes) ? input.changes : [];
      const first = changes[0] as Record<string, unknown> | undefined;
      const measure = typeof first?.measure === "string" ? first.measure : "?";
      const label = changes.length > 1 ? `${measure} +${changes.length - 1} more` : measure;
      const output = tryParseJson(item.toolOutput ?? "");
      const impactCount = Array.isArray(output?.impacts) ? output.impacts.length : null;
      const preview = impactCount !== null ? `${label} · ${impactCount} impact(s)` : label;
      return { Icon: Zap, label: "Predict Impact", preview: trunc(preview) };
    }

    // ── Domain event ──────────────────────────────────────────────────────────
    case "resolve_schema": {
      const tables = item.toolOutput ?? "Resolving schema…";
      return { Icon: Layers, label: "Schema", preview: trunc(tables) };
    }

    // ── Fallback ──────────────────────────────────────────────────────────────
    default: {
      let preview: string;
      try {
        const s = JSON.stringify(JSON.parse(item.toolInput));
        preview = s.length > 50 ? `${s.slice(0, 50)}…` : s;
      } catch {
        preview = item.toolInput.slice(0, 50);
      }
      return { Icon: Wrench, label: item.toolName, preview };
    }
  }
}

/** An expanded step's row for an item a pill also opens; same toggle, same pressed state. */
type ChildRowProps<T> = {
  item: T;
  onSelect: (item: SelectableItem) => void;
  pressed?: boolean;
};

const ArtifactChild = ({ item, onSelect, pressed }: ChildRowProps<ArtifactItem>) => {
  const { Icon, label, preview, isError } = getToolDisplay(item);

  return (
    <button
      type='button'
      aria-pressed={pressed}
      onClick={() => onSelect(item)}
      className='flex w-full items-center gap-1.5 py-0.5 text-left transition-opacity hover:opacity-70'
    >
      <Icon
        className={`h-3 w-3 shrink-0 ${isError ? "text-destructive/70" : "text-muted-foreground"}`}
      />
      <span
        className={`shrink-0 font-medium text-xs ${isError ? "text-destructive/80" : "text-muted-foreground"}`}
      >
        {label}
      </span>
      <span
        className={`flex-1 truncate text-xs ${isError ? "text-destructive/60" : "text-muted-foreground/60"}`}
      >
        {preview}
      </span>
      {item.isStreaming && <Loader2 className='h-3 w-3 shrink-0 animate-spin text-primary' />}
      {!item.isStreaming && item.durationMs !== undefined && (
        <span className='shrink-0 font-mono text-[10px] text-muted-foreground/60'>
          {formatMs(item.durationMs)}
        </span>
      )}
      {isError ? (
        <X className='h-3 w-3 shrink-0 text-destructive/70' />
      ) : (
        <ChevronRight className='h-3 w-3 shrink-0 text-muted-foreground' />
      )}
    </button>
  );
};

const SqlChild = ({ item, onSelect, pressed }: ChildRowProps<SqlItem>) => {
  const isSemantic = item.source === "semantic";
  const isVerifiedSqlFile = item.source === "verified_sql";
  const verified = isSemantic || isVerifiedSqlFile;
  const label = isSemantic ? "Semantic Query" : isVerifiedSqlFile ? "Verified Query" : "SQL Query";
  const verifiedTooltip = isVerifiedSqlFile ? VERIFIED_SQL_FILE_TOOLTIP : VERIFIED_TOOLTIP;

  return (
    <button
      type='button'
      aria-pressed={pressed}
      onClick={() => onSelect(item)}
      className='flex w-full items-center gap-1.5 py-0.5 text-left transition-opacity hover:opacity-70'
    >
      <Database
        className={cn("h-3 w-3 shrink-0", verified ? "text-primary" : "text-muted-foreground")}
      />
      <span
        className={cn(
          "flex-1 truncate font-medium text-xs",
          verified ? "text-primary" : "text-muted-foreground"
        )}
      >
        {label}
      </span>
      {verified && (
        <Tooltip>
          <TooltipTrigger asChild>
            <BadgeCheck className='h-3.5 w-3.5 shrink-0 text-oxy-blue-600 dark:text-oxy-blue-500' />
          </TooltipTrigger>
          <TooltipContent side='top'>{verifiedTooltip}</TooltipContent>
        </Tooltip>
      )}
      {item.is_preagg && (
        <Tooltip>
          <TooltipTrigger asChild>
            <Zap className='h-3.5 w-3.5 shrink-0 text-primary' />
          </TooltipTrigger>
          <TooltipContent side='top'>Served from pre-aggregation cache</TooltipContent>
        </Tooltip>
      )}
      {item.isStreaming && <Loader2 className='h-3 w-3 shrink-0 animate-spin text-primary' />}
      {!item.isStreaming && item.error && (
        <span className='shrink-0 text-destructive text-xs'>Failed: {item.error}</span>
      )}
      {!item.isStreaming && !item.error && item.rowCount !== undefined && (
        <span className='shrink-0 font-mono text-muted-foreground/60 text-xs'>
          {item.rowCount} rows
        </span>
      )}
      <ChevronRight className='h-3 w-3 shrink-0 text-muted-foreground' />
    </button>
  );
};

const AutomationChild = ({ item, onSelect, pressed }: ChildRowProps<AutomationItem>) => (
  <button
    type='button'
    aria-pressed={pressed}
    onClick={() => onSelect(item)}
    className='flex w-full items-center gap-1.5 py-0.5 text-left transition-opacity hover:opacity-70'
  >
    <GitBranch className='h-3 w-3 shrink-0 text-muted-foreground' />
    <span className='shrink-0 font-medium text-muted-foreground text-xs'>
      {item.automationName}
    </span>
    <span className='flex-1 font-mono text-muted-foreground/60 text-xs'>
      {item.stepsDone}/{item.steps.length}
    </span>
    <ChevronRight className='h-3 w-3 shrink-0 text-muted-foreground' />
  </button>
);

const TextChild = ({ item }: { item: TextItem }) => (
  <p className='whitespace-pre-wrap text-muted-foreground text-xs leading-relaxed'>
    {item.text}
    {item.isStreaming && (
      <span className='ml-0.5 inline-block h-3 w-0.5 animate-pulse bg-foreground' />
    )}
  </p>
);

// ── Step row ──────────────────────────────────────────────────────────────────

type PillInfo = {
  id: string;
  icon: React.ElementType;
  label: string;
  verified?: boolean;
  verifiedTooltip?: string;
  isPreagg?: boolean;
  item: SelectableItem;
  variant?: "default" | "builder";
};

function collectPills(items: TraceItem[]): PillInfo[] {
  const pills: PillInfo[] = [];
  for (const item of items) {
    if (item.kind === "artifact") {
      const { Icon, label } = getToolDisplay(item);
      pills.push({ id: item.id, icon: Icon, label, item });
    } else if (item.kind === "sql") {
      const isSemantic = item.source === "semantic";
      const isVerifiedSqlFile = item.source === "verified_sql";
      const verified = isSemantic || isVerifiedSqlFile;
      const label = isSemantic
        ? "Semantic Query"
        : isVerifiedSqlFile
          ? "Verified Query"
          : "SQL Query";
      const verifiedTooltip = isVerifiedSqlFile ? VERIFIED_SQL_FILE_TOOLTIP : VERIFIED_TOOLTIP;
      pills.push({
        id: item.id,
        icon: Database,
        label,
        verified,
        verifiedTooltip,
        isPreagg: item.is_preagg ?? false,
        item
      });
    } else if (item.kind === "automation") {
      pills.push({ id: item.id, icon: GitBranch, label: item.automationName, item });
    } else if (item.kind === "builder_delegation") {
      const label = item.status === "running" ? "Building…" : "Builder";
      pills.push({ id: item.id, icon: Hammer, label, item, variant: "builder" });
    }
  }
  return pills;
}

interface AnalyticsStepRowProps {
  step: AnalyticsStep;
  onSelectArtifact: (item: SelectableItem) => void;
  /** Whether an item's panel is open, for hosts where picking an item toggles its panel;
   *  shown as the pressed state of its pill and row. Omitted, they carry no state. */
  isSelected?: (item: SelectableItem) => boolean;
  /** When true, render without the bordered/bg card styling so the row blends
   *  into a flat container (used by the onboarding BuildJobsPanel). */
  flat?: boolean;
}

const AnalyticsStepRow = ({
  step,
  onSelectArtifact,
  isSelected,
  flat = false
}: AnalyticsStepRowProps) => {
  const [expanded, setExpanded] = useState(step.isStreaming);
  const colors = stepColors(step.label);
  const isRunning = step.isStreaming;
  const hasError = !!step.error;
  const isDone = !isRunning && !hasError;
  const pills = useMemo(() => collectPills(step.items), [step.items]);

  return (
    <div>
      {/* The header's toggle and the pills are siblings, never nested: a button inside a
          button is invalid HTML, and assistive tech reads the pair as one control. */}
      <div
        className={cn(
          "transition-all duration-200",
          !flat &&
            cn(
              "rounded-md border",
              isRunning
                ? cn("border-l-2", colors.border, "bg-secondary/80")
                : "border-transparent hover:bg-muted dark:hover:bg-muted/50"
            )
        )}
      >
        <button
          type='button'
          aria-expanded={expanded}
          onClick={() => setExpanded((v) => !v)}
          className={cn(
            "flex w-full cursor-pointer items-center gap-2 text-left",
            flat ? cn("pt-0.5", pills.length > 0 ? "pb-1.5" : "pb-0.5") : "px-3 py-1.5"
          )}
        >
          <div
            className={cn(
              "h-1.5 w-1.5 shrink-0 rounded-full transition-all duration-200",
              colors.dot,
              isRunning && "animate-pulse",
              !isRunning && "opacity-30"
            )}
          />
          <div className='flex min-w-0 flex-1 flex-col'>
            <span
              className={cn(
                "break-words text-sm transition-colors duration-200",
                isRunning ? "text-foreground" : "text-muted-foreground"
              )}
            >
              {step.summary || step.label}
            </span>
          </div>
          {isRunning && <Loader2 className='h-3 w-3 shrink-0 animate-spin text-info' />}
          {isDone && <Check className='h-3 w-3 shrink-0 text-status-success-text' />}
          {hasError && <span className='shrink-0 font-medium text-destructive text-xs'>Error</span>}
          {isDone && step.llmUsage && (
            <Tooltip>
              <TooltipTrigger asChild onClick={(e) => e.stopPropagation()}>
                <Info className='h-3 w-3 shrink-0 cursor-pointer text-muted-foreground/50 hover:text-muted-foreground' />
              </TooltipTrigger>
              <TooltipContent side='left' className='max-w-xs'>
                <LlmUsageTooltip usage={step.llmUsage} />
              </TooltipContent>
            </Tooltip>
          )}
          <ChevronRight
            className={cn(
              "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
              expanded && "rotate-90"
            )}
          />
        </button>
        {pills.length > 0 && (
          <div
            className={cn(
              "flex flex-wrap items-center gap-1 pl-3.5",
              flat ? "pb-0.5" : "mx-3 mb-1.5"
            )}
          >
            {pills.map((pill) => (
              <ArtifactPill
                key={pill.id}
                icon={pill.icon}
                label={pill.label}
                verified={pill.verified}
                verifiedTooltip={pill.verifiedTooltip}
                isPreagg={pill.isPreagg}
                onClick={() => onSelectArtifact(pill.item)}
                variant={pill.variant}
                pressed={isSelected?.(pill.item)}
              />
            ))}
          </div>
        )}
      </div>

      <div
        className={cn(
          "overflow-hidden transition-all duration-300",
          expanded ? "opacity-100" : "max-h-0 opacity-0"
        )}
      >
        {step.items.length > 0 && (
          <div className={cn("mt-1 mb-2 ml-8 space-y-1.5 border-l-2 pl-3", colors.border)}>
            {step.items.map((item) => {
              if (item.kind === "thinking")
                return <ThinkingChild key={item.id} item={item} border={colors.border} />;
              if (item.kind === "artifact")
                return (
                  <ArtifactChild
                    key={item.id}
                    item={item}
                    onSelect={onSelectArtifact}
                    pressed={isSelected?.(item)}
                  />
                );
              if (item.kind === "sql")
                return (
                  <SqlChild
                    key={item.id}
                    item={item}
                    onSelect={onSelectArtifact}
                    pressed={isSelected?.(item)}
                  />
                );
              if (item.kind === "automation")
                return item.isStreaming ? (
                  <AutomationDelegationCard
                    key={item.id}
                    item={item}
                    onSelect={onSelectArtifact}
                    pressed={isSelected?.(item)}
                  />
                ) : (
                  <AutomationChild
                    key={item.id}
                    item={item}
                    onSelect={onSelectArtifact}
                    pressed={isSelected?.(item)}
                  />
                );
              if (item.kind === "builder_delegation")
                return (
                  <BuilderDelegationCard
                    key={item.id}
                    item={item}
                    onSelect={onSelectArtifact}
                    pressed={isSelected?.(item)}
                  />
                );
              return <TextChild key={item.id} item={item} />;
            })}
          </div>
        )}
        {step.error && <p className='mt-1 ml-8 text-destructive text-xs'>{step.error}</p>}
      </div>
    </div>
  );
};

export default AnalyticsStepRow;
