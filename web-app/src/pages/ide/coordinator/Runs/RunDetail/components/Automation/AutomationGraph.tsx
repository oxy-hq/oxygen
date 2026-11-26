import {
  Bot,
  ChevronDown,
  ChevronRight,
  Database,
  Flame,
  GitBranch,
  Repeat,
  Wrench
} from "lucide-react";
import type React from "react";
import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { formatDurationMs } from "../../../../components/utils";
import { TimeAxis } from "../TimeAxis";
import type { AutomationModel, AutomationNode, AutomationNodeKind } from "./model";

// Kind is conveyed by the icon's shape (Database / Bot / GitBranch /
// Repeat / Wrench) — each one is visually distinct. Coloring the
// icon AND the bar by kind on top of that was rainbow-encoding the
// same fact, and the per-kind hues (emerald SQL / fuchsia
// automation / cyan loop) collided with the status palette
// (emerald = success). One primary tint for every bar; status comes
// from the border + dot; structure comes from the icon.
const KIND_META: Record<
  AutomationNodeKind,
  { icon: React.ElementType; label: string; tone: string; barTone: string }
> = {
  sql: {
    icon: Database,
    label: "SQL",
    tone: "text-muted-foreground",
    barTone: "bg-primary/70"
  },
  agent: {
    icon: Bot,
    label: "Agent",
    tone: "text-muted-foreground",
    barTone: "bg-primary/70"
  },
  automation: {
    icon: GitBranch,
    label: "Sub-automation",
    tone: "text-muted-foreground",
    barTone: "bg-primary/70"
  },
  loop: {
    icon: Repeat,
    label: "Loop",
    tone: "text-muted-foreground",
    barTone: "bg-primary/70"
  },
  generic: {
    icon: Wrench,
    label: "Step",
    tone: "text-muted-foreground",
    barTone: "bg-primary/70"
  }
};

const STATUS_BORDER: Record<string, string> = {
  succeeded: "border-dot-success",
  cached: "border-dot-info",
  failed: "border-destructive",
  running: "border-primary",
  pending: "border-border border-dashed"
};

const STATUS_BG: Record<string, string> = {
  succeeded: "bg-dot-success",
  cached: "bg-dot-info",
  failed: "bg-destructive",
  running: "bg-primary animate-pulse",
  pending: "bg-muted-foreground/30"
};

/**
 * Hierarchical DAG renderer. Tree comes from the automation's
 * `subrun_started` event payload (`steps[].inner_tasks` recursively),
 * so container types (`loop_sequential`, `automation`) render with
 * nested children indented underneath. The critical leaf glows even
 * when it lives deep inside a container.
 *
 * Pure CSS layout — vertical chain + tree-style indent for children.
 * No graph library; the YAML defines tasks sequentially, so the
 * dataflow shape is "chain with collapsible sub-chains" rather than a
 * general DAG, and CSS handles that perfectly.
 */
export const AutomationGraph: React.FC<{
  model: AutomationModel;
  selectedName: string | null;
  onSelect: (node: AutomationNode) => void;
}> = ({ model, selectedName, onSelect }) => {
  if (model.nodes.length === 0) {
    return (
      <div className='px-4 py-10 text-center text-muted-foreground text-sm'>
        No steps captured for this automation run yet.
      </div>
    );
  }

  const criticalName = model.criticalNode?.name ?? null;
  const timeWindow = model.window;

  return (
    <div className='mx-auto flex max-w-3xl flex-col items-stretch gap-0 p-4'>
      {timeWindow && <TimeAxis spanMs={timeWindow.spanMs} />}
      {model.nodes.map((node, i) => (
        <div key={node.key} className='flex flex-col'>
          <StepBranch
            node={node}
            depth={0}
            criticalName={criticalName}
            selectedName={selectedName}
            onSelect={onSelect}
            timeWindow={timeWindow}
          />
          {i < model.nodes.length - 1 && <Connector />}
        </div>
      ))}
    </div>
  );
};

type GraphWindow = { t0Ms: number; t1Ms: number; spanMs: number } | null;

const StepBranch: React.FC<{
  node: AutomationNode;
  depth: number;
  criticalName: string | null;
  selectedName: string | null;
  onSelect: (n: AutomationNode) => void;
  timeWindow: GraphWindow;
}> = ({ node, depth, criticalName, selectedName, onSelect, timeWindow }) => {
  const [expanded, setExpanded] = useState(true);
  const hasChildren = node.children.length > 0;

  return (
    <div className='flex flex-col'>
      <StepCard
        node={node}
        depth={depth}
        isCritical={criticalName === node.name && node.children.length === 0}
        isSelected={selectedName === node.name}
        canExpand={hasChildren}
        expanded={expanded}
        onToggleExpand={() => setExpanded((v) => !v)}
        onClick={() => onSelect(node)}
        timeWindow={timeWindow}
      />
      {hasChildren && expanded && (
        <div className='relative mt-1 ml-6 border-border border-l-2 pl-3'>
          {node.children.map((child, i) => (
            <div key={child.key} className='flex flex-col'>
              <StepBranch
                node={child}
                depth={depth + 1}
                criticalName={criticalName}
                selectedName={selectedName}
                onSelect={onSelect}
                timeWindow={timeWindow}
              />
              {i < node.children.length - 1 && <Connector inset />}
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

const StepCard: React.FC<{
  node: AutomationNode;
  depth: number;
  isCritical: boolean;
  isSelected: boolean;
  canExpand: boolean;
  expanded: boolean;
  onToggleExpand: () => void;
  onClick: () => void;
  timeWindow: GraphWindow;
}> = ({
  node,
  isCritical,
  isSelected,
  canExpand,
  expanded,
  onToggleExpand,
  onClick,
  timeWindow
}) => {
  const meta = KIND_META[node.kind];
  const KindIcon = meta.icon;
  const borderTone = STATUS_BORDER[node.status] ?? "border-border";
  const statusDot = STATUS_BG[node.status] ?? "bg-muted-foreground";

  // Position the proportional time bar on the shared axis. Pending /
  // synthetic-parent rows (no startedAt) render an empty track so the
  // tree shape stays readable while the bar columns stay aligned.
  let barLeftPct = 0;
  let barWidthPct = 0;
  if (timeWindow && node.startedAt) {
    const startedMs = new Date(node.startedAt).getTime();
    const endedMs = node.completedAt
      ? new Date(node.completedAt).getTime()
      : node.durationMs
        ? startedMs + node.durationMs
        : startedMs;
    if (Number.isFinite(startedMs) && timeWindow.spanMs > 0) {
      barLeftPct = Math.max(0, ((startedMs - timeWindow.t0Ms) / timeWindow.spanMs) * 100);
      barWidthPct = Math.max(((endedMs - startedMs) / timeWindow.spanMs) * 100, 0.4);
    }
  }

  const failed = node.status === "failed";
  const running = node.status === "running";

  return (
    <button
      type='button'
      onClick={onClick}
      data-testid='automation-step-card'
      data-step-name={node.name}
      className={cn(
        "group w-full border-2 bg-card px-3 py-2 text-left transition-colors",
        borderTone,
        isSelected && "shadow-md ring-2 ring-ring",
        isCritical && "ring-2 ring-dot-warning/60"
      )}
    >
      <div className='flex items-center gap-2'>
        {canExpand ? (
          <button
            type='button'
            onClick={(e) => {
              e.stopPropagation();
              onToggleExpand();
            }}
            className='shrink-0 p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground'
            aria-label={expanded ? "Collapse children" : "Expand children"}
          >
            {expanded ? (
              <ChevronDown className='h-3.5 w-3.5' />
            ) : (
              <ChevronRight className='h-3.5 w-3.5' />
            )}
          </button>
        ) : (
          <span className='h-3.5 w-3.5 shrink-0' />
        )}
        <span className={cn("h-2.5 w-2.5 shrink-0 rounded-full", statusDot)} />
        <KindIcon className={cn("h-4 w-4 shrink-0", meta.tone)} />
        <span className='min-w-0 flex-1 truncate font-medium text-sm'>{node.name}</span>
        {isCritical && (
          <span className='flex items-center gap-1 text-status-warning text-xs'>
            <Flame className='h-3 w-3' /> critical
          </span>
        )}
        {node.cached && (
          <span className='bg-dot-info/15 px-1.5 py-0.5 text-status-info text-xs'>cached</span>
        )}
        <span className='text-muted-foreground text-xs tabular-nums'>
          {node.durationMs !== null
            ? formatDurationMs(node.durationMs)
            : node.status === "pending"
              ? "—"
              : "…"}
        </span>
      </div>

      {/* Proportional time bar — when there's a usable timeWindow, this is
          the unified Gantt encoding. Width = share of total run timeWindow;
          left offset = absolute start position. Pending / synthetic
          rows render only the empty track so columns stay aligned. */}
      {timeWindow && (
        <div className='mt-1.5 ml-10 h-1.5 bg-muted/40'>
          {barWidthPct > 0 && (
            <div
              className={cn(
                "h-full ",
                failed ? "bg-destructive/70" : meta.barTone,
                running && "animate-pulse",
                isCritical && "ring-1 ring-dot-warning"
              )}
              style={{
                marginLeft: `${barLeftPct}%`,
                width: `${barWidthPct}%`
              }}
            />
          )}
        </div>
      )}

      <div className='mt-1 flex items-center gap-2 pl-10 text-muted-foreground text-xs'>
        <span>{meta.label}</span>
        {node.children.length > 0 && (
          <span className='tabular-nums'>
            · {node.children.length}{" "}
            {node.kind === "loop"
              ? `iteration${node.children.length === 1 ? "" : "s"}`
              : `child step${node.children.length === 1 ? "" : "s"}`}
          </span>
        )}
        {node.kind === "agent" && node.nestedWaterfall && (
          <span className='tabular-nums'>· {node.nestedWaterfall.phases.length} phases</span>
        )}
        {node.kind === "sql" && node.query?.success && (
          <span className='tabular-nums'>· {node.query.rowCount.toLocaleString()} rows</span>
        )}
        {node.error && <span className='truncate text-destructive italic'>· {node.error}</span>}
      </div>
    </button>
  );
};

const Connector: React.FC<{ inset?: boolean }> = ({ inset }) => (
  <div className={cn("h-4 w-px bg-border", inset ? "ml-3" : "self-center")} aria-hidden='true' />
);
