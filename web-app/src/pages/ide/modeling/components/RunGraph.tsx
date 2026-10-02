import {
  Background,
  BackgroundVariant,
  type Edge,
  Handle,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow
} from "@xyflow/react";
import ELK from "elkjs";
import { ChevronDown, ChevronUp, Loader2, XCircle } from "lucide-react";
import type React from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { ErrorBoundary } from "react-error-boundary";
import ErrorAlert from "@/components/ui/ErrorAlert";
import useModelingLineage from "@/hooks/api/modeling/useModelingLineage";
import type { RunStreamState } from "@/hooks/api/modeling/useModelingRunStream";
import { cn } from "@/libs/shadcn/utils";
import type { LineageEdge, LineageNode } from "@/types/modeling";

const elk = new ELK();

const HANDLE_HIDDEN = {
  width: 0,
  height: 0,
  minWidth: 0,
  minHeight: 0,
  opacity: 0,
  border: "none",
  background: "transparent",
  padding: 0
} as const;

const NODE_W = 180;
const NODE_H = 36;

type NodeRunStatus = "pending" | "running" | "success" | "error";

interface NodeRunInfo {
  status: NodeRunStatus;
  duration_ms?: number;
  message?: string;
}

const STATUS_STYLES: Record<
  NodeRunStatus,
  { border: string; bg: string; text: string; badge: string }
> = {
  pending: {
    border: "border-border",
    bg: "bg-background",
    text: "text-muted-foreground",
    badge: "pending"
  },
  running: {
    border: "border-primary",
    bg: "bg-primary/10",
    text: "text-primary",
    badge: "running"
  },
  success: {
    border: "border-emerald-500/60",
    bg: "bg-emerald-500/10",
    text: "text-emerald-600 dark:text-emerald-400",
    badge: "ok"
  },
  error: {
    border: "border-destructive/60",
    bg: "bg-destructive/10",
    text: "text-destructive",
    badge: "error"
  }
};

// Source nodes are never executed by dbt run — show them in their natural style
const SOURCE_STYLES: Record<string, { border: string; bg: string; text: string; badge: string }> = {
  source: {
    border: "border-emerald-500/60",
    bg: "bg-emerald-500/10",
    text: "text-emerald-600 dark:text-emerald-400",
    badge: "source"
  },
  seed: {
    border: "border-amber-500/60",
    bg: "bg-amber-500/10",
    text: "text-amber-600 dark:text-amber-400",
    badge: "seed"
  },
  snapshot: {
    border: "border-purple-500/60",
    bg: "bg-purple-500/10",
    text: "text-purple-600 dark:text-purple-400",
    badge: "snapshot"
  }
};

function deriveInfoMap(runStream: RunStreamState): Map<string, NodeRunInfo> {
  const map = new Map<string, NodeRunInfo>();
  if (runStream.phase === "idle") return map;

  for (const event of runStream.events) {
    if (event.kind === "node_started") {
      map.set(event.unique_id, { status: "running" });
    } else if (event.kind === "node_completed") {
      const ok =
        event.status === "SUCCESS" ||
        event.status === "success" ||
        event.status === "PASS" ||
        event.status === "pass";
      const skipped =
        event.status === "SKIP" || event.status === "skipped" || event.status === "SKIPPED";
      map.set(event.unique_id, {
        status: ok || skipped ? "success" : "error",
        duration_ms: event.duration_ms,
        message: skipped ? undefined : (event.message ?? undefined)
      });
    }
  }
  return map;
}

function RunFlowNode({ data }: NodeProps) {
  const { label, status, resourceType, duration_ms } = data as {
    label: string;
    status: NodeRunStatus;
    resourceType: string;
    duration_ms?: number;
  };

  // Non-model resource types are never executed — use their natural style when pending
  const isPendingNonRunnable = status === "pending" && resourceType !== "model";
  const style = isPendingNonRunnable
    ? (SOURCE_STYLES[resourceType] ?? STATUS_STYLES.pending)
    : STATUS_STYLES[status];

  const badge =
    status === "success" && duration_ms != null
      ? `${duration_ms}ms`
      : status === "error" && duration_ms != null
        ? `error · ${duration_ms}ms`
        : style.badge;

  return (
    <div
      className={cn(
        "flex items-center gap-1.5 rounded border px-2.5 py-1 font-medium text-xs",
        style.border,
        style.bg
      )}
      style={{ width: NODE_W }}
    >
      <Handle type='target' position={Position.Left} style={HANDLE_HIDDEN} />
      {status === "running" && <Loader2 className='h-3 w-3 shrink-0 animate-spin text-primary' />}
      <span className={cn("min-w-0 truncate", style.text)}>{label}</span>
      <span
        className={cn(
          "ml-auto shrink-0 rounded border px-1 py-0.5 text-[10px] opacity-80",
          style.border,
          style.text
        )}
      >
        {badge}
      </span>
      <Handle type='source' position={Position.Right} style={HANDLE_HIDDEN} />
    </div>
  );
}

const nodeTypes = { run: RunFlowNode };

const ExpandableMessage: React.FC<{ message: string }> = ({ message }) => {
  const [expanded, setExpanded] = useState(false);
  const isLong = message.split("\n").length > 3 || message.length > 200;
  return (
    <div>
      <p
        className={cn(
          "mt-0.5 whitespace-pre-wrap break-all text-destructive/80",
          !expanded && isLong && "line-clamp-3"
        )}
      >
        {message}
      </p>
      {isLong && (
        <button
          type='button'
          onClick={(e) => {
            e.stopPropagation();
            setExpanded((v) => !v);
          }}
          className='mt-0.5 text-muted-foreground underline hover:text-foreground'
        >
          {expanded ? "Show less ↑" : "Show more ↓"}
        </button>
      )}
    </div>
  );
};

function FitViewOnResize({
  containerRef
}: {
  containerRef: React.RefObject<HTMLDivElement | null>;
}) {
  const { fitView } = useReactFlow();

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => {
      // React Flow's `fitView` only ever resolves (to whether it fitted).
      void fitView({ padding: 0.25 });
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [containerRef, fitView]);

  return null;
}

async function computeLayout(
  nodes: LineageNode[],
  edges: LineageEdge[]
): Promise<{ baseNodes: Node[]; flowEdges: Edge[] }> {
  const elkGraph = {
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "RIGHT",
      "elk.layered.spacing.nodeNodeBetweenLayers": "80",
      "elk.spacing.nodeNode": "16",
      "elk.layered.crossingMinimization.strategy": "LAYER_SWEEP",
      "elk.layered.nodePlacement.strategy": "BRANDES_KOEPF",
      "elk.edgeRouting": "ORTHOGONAL"
    },
    children: nodes.map((n) => ({ id: n.unique_id, width: NODE_W, height: NODE_H })),
    edges: edges.map((e, i) => ({ id: `e-${i}`, sources: [e.source], targets: [e.target] }))
  };

  const layout = await elk.layout(elkGraph);

  const baseNodes: Node[] = (layout.children ?? []).map((child) => {
    const node = nodes.find((n) => n.unique_id === child.id)!;
    return {
      id: child.id,
      type: "run",
      data: {
        label: node.name,
        resourceType: node.resource_type,
        status: "pending" as NodeRunStatus,
        duration_ms: undefined,
        message: undefined
      },
      position: { x: child.x ?? 0, y: child.y ?? 0 }
    };
  });

  const flowEdges: Edge[] = edges.map((e, i) => ({
    id: `${e.source}->${e.target}-${i}`,
    source: e.source,
    target: e.target,
    style: { stroke: "var(--muted-foreground)", strokeWidth: 1, opacity: 0.4 }
  }));

  return { baseNodes, flowEdges };
}

function collectVisible(
  selectedId: string,
  allNodes: LineageNode[],
  allEdges: LineageEdge[]
): { visibleNodes: LineageNode[]; visibleEdges: LineageEdge[] } {
  const visibleIds = new Set<string>([selectedId]);

  let frontier = [selectedId];
  while (frontier.length > 0) {
    const next: string[] = [];
    for (const nodeId of frontier) {
      for (const edge of allEdges) {
        if (edge.target === nodeId && !visibleIds.has(edge.source)) {
          visibleIds.add(edge.source);
          next.push(edge.source);
        }
      }
    }
    frontier = next;
  }

  return {
    visibleNodes: allNodes.filter((n) => visibleIds.has(n.unique_id)),
    visibleEdges: allEdges.filter((e) => visibleIds.has(e.source) && visibleIds.has(e.target))
  };
}

interface RunGraphProps {
  dbtProjectName: string;
  runStream: RunStreamState;
  selectedNodeId?: string;
}

function RunGraphInner({ dbtProjectName, runStream, selectedNodeId }: RunGraphProps) {
  const { data, isLoading, error } = useModelingLineage(dbtProjectName);
  const [baseNodes, setBaseNodes] = useState<Node[]>([]);
  const [flowEdges, setFlowEdges] = useState<Edge[]>([]);
  const [layoutPending, setLayoutPending] = useState(false);
  const [layoutFailed, setLayoutFailed] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!data) {
      // Nothing to lay out. A layout cancelled below no longer clears this itself.
      setLayoutPending(false);
      return;
    }
    const { visibleNodes, visibleEdges } = selectedNodeId
      ? collectVisible(selectedNodeId, data.nodes, data.edges)
      : { visibleNodes: data.nodes, visibleEdges: data.edges };
    // A layout still running for a previous selection belongs to a selection that is
    // gone: its graph, its failure and its "finished" must not land on this one.
    let cancelled = false;
    setLayoutPending(true);
    setLayoutFailed(false);
    computeLayout(visibleNodes, visibleEdges)
      .then(({ baseNodes: bn, flowEdges: fe }) => {
        if (cancelled) return;
        setBaseNodes(bn);
        setFlowEdges(fe);
      })
      // ELK rejects a graph it cannot lay out (e.g. an edge naming a node the lineage
      // does not list). Say so, rather than leave the previous selection's graph on screen.
      .catch((err) => {
        console.error("Failed to lay out run graph:", err);
        if (!cancelled) setLayoutFailed(true);
      })
      .finally(() => {
        if (!cancelled) setLayoutPending(false);
      });
    return () => {
      cancelled = true;
    };
  }, [data, selectedNodeId]);

  const infoMap = useMemo(() => deriveInfoMap(runStream), [runStream]);

  const flowNodes = useMemo(
    () =>
      baseNodes.map((n) => {
        const info = infoMap.get(n.id);
        return {
          ...n,
          data: {
            ...n.data,
            status: info?.status ?? "pending",
            duration_ms: info?.duration_ms,
            message: info?.message
          }
        };
      }),
    [baseNodes, infoMap]
  );

  const errorNodes = useMemo(
    () => flowNodes.filter((n) => n.data.status === "error" && n.data.message),
    [flowNodes]
  );
  const [errorsExpanded, setErrorsExpanded] = useState(false);

  const globalError = runStream.phase === "error" ? runStream.message : null;

  if (isLoading || layoutPending) {
    return (
      <div className='flex h-full items-center justify-center'>
        <Loader2 className='h-5 w-5 animate-spin text-muted-foreground' />
      </div>
    );
  }

  if (error || layoutFailed) {
    return (
      <div className='flex h-full items-center justify-center text-destructive text-sm'>
        Failed to load lineage
      </div>
    );
  }

  if (flowNodes.length === 0) {
    return (
      <div className='flex h-full items-center justify-center text-muted-foreground text-sm'>
        No lineage data available
      </div>
    );
  }

  return (
    <div className='flex h-full flex-col'>
      {globalError && (
        <div className='flex shrink-0 items-start gap-2 border-b bg-destructive/5 px-3 py-2 font-mono text-destructive text-xs'>
          <XCircle className='mt-0.5 h-3 w-3 shrink-0' />
          <span className='whitespace-pre-wrap break-all'>{globalError}</span>
        </div>
      )}
      <div ref={containerRef} className='min-h-0 flex-1'>
        <ReactFlow
          nodes={flowNodes}
          edges={flowEdges}
          nodeTypes={nodeTypes}
          fitView
          fitViewOptions={{ padding: 0.25 }}
          elementsSelectable={false}
          nodesConnectable={false}
          nodesDraggable={false}
          minZoom={0.2}
          maxZoom={2}
          proOptions={{ hideAttribution: true }}
          style={{ background: "var(--background)" }}
        >
          <Background color='var(--muted-foreground)' variant={BackgroundVariant.Dots} />
          <FitViewOnResize containerRef={containerRef} />
        </ReactFlow>
      </div>

      {errorNodes.length > 0 && (
        <div className='shrink-0 border-t'>
          <button
            type='button'
            onClick={() => setErrorsExpanded((v) => !v)}
            className='flex w-full items-center gap-1.5 bg-destructive/5 px-3 py-1.5 font-mono text-xs hover:bg-destructive/10'
          >
            <XCircle className='h-3 w-3 shrink-0 text-destructive' />
            <span className='text-destructive'>
              {errorNodes.length} error{errorNodes.length !== 1 ? "s" : ""}
            </span>
            <span className='ml-auto text-muted-foreground'>
              {errorsExpanded ? (
                <ChevronDown className='h-3 w-3' />
              ) : (
                <ChevronUp className='h-3 w-3' />
              )}
            </span>
          </button>
          {errorsExpanded && (
            <div className='max-h-28 overflow-y-auto bg-destructive/5'>
              {errorNodes.map((n) => {
                const d = n.data as unknown as { label: string; message: string };
                return (
                  <div key={n.id} className='flex gap-2 border-t px-3 py-2'>
                    <div className='min-w-0 font-mono text-xs'>
                      <span className='font-semibold text-destructive'>{d.label}</span>
                      <ExpandableMessage message={d.message} />
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

const RunGraph: React.FC<RunGraphProps> = (props) => (
  <ReactFlowProvider>
    <ErrorBoundary
      resetKeys={[props.dbtProjectName, props.selectedNodeId]}
      fallback={
        <div className='flex h-full w-full items-center justify-center p-4'>
          <ErrorAlert
            title='Failed to render run graph'
            message='The run graph could not be drawn. Try refreshing the page.'
          />
        </div>
      }
    >
      <RunGraphInner {...props} />
    </ErrorBoundary>
  </ReactFlowProvider>
);

export default RunGraph;
