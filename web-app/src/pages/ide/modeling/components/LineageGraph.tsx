import {
  Background,
  BackgroundVariant,
  type Edge,
  Handle,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import ELK from "elkjs";
import { Loader2 } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { ErrorBoundary } from "react-error-boundary";
import ErrorAlert from "@/components/ui/ErrorAlert";
import useModelingLineage from "@/hooks/api/modeling/useModelingLineage";
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

// Visual style per dbt resource type
const RESOURCE_STYLES: Record<string, { badge: string; border: string; bg: string; text: string }> =
  {
    model: {
      badge: "model",
      border: "border-border",
      bg: "bg-background",
      text: "text-foreground"
    },
    source: {
      badge: "source",
      border: "border-emerald-500/60",
      bg: "bg-emerald-500/10",
      text: "text-emerald-600 dark:text-emerald-400"
    },
    seed: {
      badge: "seed",
      border: "border-amber-500/60",
      bg: "bg-amber-500/10",
      text: "text-amber-600 dark:text-amber-400"
    },
    snapshot: {
      badge: "snapshot",
      border: "border-purple-500/60",
      bg: "bg-purple-500/10",
      text: "text-purple-600 dark:text-purple-400"
    }
  };

function getResourceStyle(resourceType: string) {
  return RESOURCE_STYLES[resourceType] ?? RESOURCE_STYLES.model;
}

function LineageFlowNode({ data }: NodeProps) {
  const { label, resourceType, isSelected } = data as {
    label: string;
    resourceType: string;
    isSelected: boolean;
  };
  const style = getResourceStyle(resourceType);
  return (
    <div
      className={cn(
        "flex items-center gap-1.5 rounded border px-2.5 py-1 font-medium text-xs",
        isSelected ? "border-primary bg-primary/10 text-primary" : cn(style.border, style.bg)
      )}
      style={{ width: NODE_W }}
    >
      <Handle type='target' position={Position.Left} style={HANDLE_HIDDEN} />
      <span className={cn("min-w-0 truncate", isSelected ? "text-primary" : style.text)}>
        {label}
      </span>
      <span
        className={cn(
          "ml-auto shrink-0 rounded px-1 py-0.5 text-[10px]",
          isSelected
            ? "bg-primary/20 text-primary"
            : cn(style.border, "border", style.text, "opacity-80")
        )}
      >
        {style.badge}
      </span>
      <Handle type='source' position={Position.Right} style={HANDLE_HIDDEN} />
    </div>
  );
}

const nodeTypes = { lineage: LineageFlowNode };

function collectVisible(
  selectedId: string,
  allNodes: LineageNode[],
  allEdges: LineageEdge[]
): { visibleNodes: LineageNode[]; visibleEdges: LineageEdge[] } {
  const visibleIds = new Set<string>([selectedId]);

  // BFS upstream
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

  // BFS downstream
  frontier = [selectedId];
  while (frontier.length > 0) {
    const next: string[] = [];
    for (const nodeId of frontier) {
      for (const edge of allEdges) {
        if (edge.source === nodeId && !visibleIds.has(edge.target)) {
          visibleIds.add(edge.target);
          next.push(edge.target);
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

async function computeLayout(
  selectedId: string,
  visibleNodes: LineageNode[],
  visibleEdges: LineageEdge[]
): Promise<{ flowNodes: Node[]; flowEdges: Edge[] }> {
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
    children: visibleNodes.map((n) => ({
      id: n.unique_id,
      width: NODE_W,
      height: NODE_H
    })),
    edges: visibleEdges.map((e, i) => ({
      id: `e-${i}`,
      sources: [e.source],
      targets: [e.target]
    }))
  };

  const layout = await elk.layout(elkGraph);

  const flowNodes: Node[] = (layout.children ?? []).map((child) => {
    const node = visibleNodes.find((n) => n.unique_id === child.id)!;
    return {
      id: child.id,
      type: "lineage",
      data: {
        label: node.name,
        resourceType: node.resource_type,
        isSelected: child.id === selectedId
      },
      position: { x: child.x ?? 0, y: child.y ?? 0 }
    };
  });

  const flowEdges: Edge[] = visibleEdges.map((e, i) => ({
    id: `${e.source}->${e.target}-${i}`,
    source: e.source,
    target: e.target,
    style: {
      stroke: "var(--muted-foreground)",
      strokeWidth: 1,
      opacity: 0.4
    }
  }));

  return { flowNodes, flowEdges };
}

interface LineageGraphProps {
  nodeId: string;
  dbtProjectName: string;
}

function LineageGraphInner({ nodeId, dbtProjectName }: LineageGraphProps) {
  const { data, isLoading, error } = useModelingLineage(dbtProjectName);
  const [flowNodes, setFlowNodes] = useState<Node[]>([]);
  const [flowEdges, setFlowEdges] = useState<Edge[]>([]);
  const [layoutPending, setLayoutPending] = useState(false);
  const [layoutFailed, setLayoutFailed] = useState(false);

  useEffect(() => {
    if (!data) {
      // Nothing to lay out. A layout cancelled below no longer clears this itself.
      setLayoutPending(false);
      return;
    }
    const { visibleNodes, visibleEdges } = collectVisible(nodeId, data.nodes, data.edges);
    // A layout still running for a previous node belongs to a selection that is gone:
    // its graph, its failure and its "finished" must not land on this one.
    let cancelled = false;
    setLayoutPending(true);
    setLayoutFailed(false);
    computeLayout(nodeId, visibleNodes, visibleEdges)
      .then(({ flowNodes: fn, flowEdges: fe }) => {
        if (cancelled) return;
        setFlowNodes(fn);
        setFlowEdges(fe);
      })
      // ELK rejects a graph it cannot lay out (e.g. an edge naming a node the lineage
      // does not list). Say so, rather than leave the previous node's graph on screen.
      .catch((err) => {
        console.error("Failed to lay out lineage graph:", err);
        if (!cancelled) setLayoutFailed(true);
      })
      .finally(() => {
        if (!cancelled) setLayoutPending(false);
      });
    return () => {
      cancelled = true;
    };
  }, [nodeId, data]);

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
    </ReactFlow>
  );
}

const LineageGraph: React.FC<LineageGraphProps> = (props) => (
  <ReactFlowProvider>
    <ErrorBoundary
      resetKeys={[props.nodeId, props.dbtProjectName]}
      fallback={
        <div className='flex h-full w-full items-center justify-center p-4'>
          <ErrorAlert
            title='Failed to render lineage graph'
            message='The lineage could not be drawn. Try refreshing the page.'
          />
        </div>
      }
    >
      <LineageGraphInner {...props} />
    </ErrorBoundary>
  </ReactFlowProvider>
);

export default LineageGraph;
