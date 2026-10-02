import {
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  Handle,
  type Node,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesState
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { type ComponentType, useEffect, useMemo, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { layoutTree } from "./elkLayout";

type NodeTone = "root" | "default" | "muted";

export interface RelationshipNodeData extends Record<string, unknown> {
  label: string;
  sublabel?: string;
  tone?: NodeTone;
  icon?: ComponentType<{ className?: string }>;
  onClick?: () => void;
}

/** A single card node — the same look across every relationship chart. */
function RelationshipNode({ data }: NodeProps<Node<RelationshipNodeData>>) {
  const { label, sublabel, tone = "default", icon: Icon, onClick } = data;
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: a React Flow node is a div; when clickable it carries role/tabIndex/onKeyDown for keyboard access.
    <div
      className={cn(
        "min-w-40 max-w-64 rounded-lg border bg-card px-3 py-2 shadow-sm transition-colors",
        tone === "root" && "border-primary/40",
        tone === "muted" && "border-border/60 bg-muted/40",
        tone === "default" && "border-border",
        onClick && "cursor-pointer hover:border-primary/50"
      )}
      onClick={onClick}
      onKeyDown={onClick ? (e) => e.key === "Enter" && onClick() : undefined}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
    >
      <Handle type='target' position={Position.Top} className='!bg-border !size-1.5 !border-0' />
      <div className='flex items-center gap-1.5'>
        {Icon && <Icon className='size-3.5 shrink-0 text-muted-foreground' />}
        <span className='truncate font-medium text-sm'>{label}</span>
      </div>
      {sublabel && <div className='truncate text-muted-foreground text-xs'>{sublabel}</div>}
      <Handle type='source' position={Position.Bottom} className='!bg-border !size-1.5 !border-0' />
    </div>
  );
}

const nodeTypes = { relationship: RelationshipNode };

function Chart({
  nodes: inputNodes,
  edges: inputEdges,
  height
}: {
  nodes: Array<{ id: string; data: RelationshipNodeData }>;
  edges: Array<{ source: string; target: string }>;
  height: number | string;
}) {
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<RelationshipNodeData>>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [layoutFailed, setLayoutFailed] = useState(false);

  // The raw graph, before layout. Memoized on the input identity so a parent
  // re-render doesn't re-run ELK unless the shape actually changed.
  const rawNodes = useMemo<Node<RelationshipNodeData>[]>(
    () =>
      inputNodes.map((n) => ({
        id: n.id,
        type: "relationship",
        position: { x: 0, y: 0 },
        data: n.data
      })),
    [inputNodes]
  );
  const rawEdges = useMemo<Edge[]>(
    () =>
      inputEdges.map((e, i) => ({
        id: `${e.source}->${e.target}-${i}`,
        source: e.source,
        target: e.target,
        type: "smoothstep",
        style: { stroke: "var(--border)", strokeWidth: 1.5 }
      })),
    [inputEdges]
  );

  useEffect(() => {
    let cancelled = false;
    layoutTree(rawNodes, rawEdges)
      .then((laidOut) => {
        if (cancelled) return;
        setNodes(laidOut);
        setEdges(rawEdges);
        setLayoutFailed(false);
      })
      // ELK rejects a graph it cannot lay out, such as an edge naming a node
      // that is not in it. Say so, rather than leave the chart blank or showing
      // what it last drew.
      .catch((error: unknown) => {
        console.error("Failed to lay out the relationship chart:", error);
        if (!cancelled) setLayoutFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [rawNodes, rawEdges, setNodes, setEdges]);

  if (layoutFailed) {
    return (
      <div
        style={{ height }}
        className='flex w-full items-center justify-center rounded-lg border bg-muted/20 text-destructive text-sm'
      >
        Failed to draw the chart
      </div>
    );
  }

  return (
    <div style={{ height }} className='w-full overflow-hidden rounded-lg border bg-muted/20'>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        nodeTypes={nodeTypes}
        fitView
        fitViewOptions={{ padding: 0.2 }}
        minZoom={0.2}
        maxZoom={1.5}
        proOptions={{ hideAttribution: true }}
        nodesConnectable={false}
        edgesFocusable={false}
      >
        <Background color='var(--ring)' variant={BackgroundVariant.Dots} gap={16} size={1} />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  );
}

/**
 * A small, interactive relationship graph — pan/zoom, ELK-laid-out top-down, one
 * card style. Wrap-and-go: pass nodes + edges. Used for the partner org chart and
 * org-membership charts so they read as one product, not hand-rolled CSS trees.
 */
export default function RelationshipChart(props: {
  nodes: Array<{ id: string; data: RelationshipNodeData }>;
  edges: Array<{ source: string; target: string }>;
  height?: number | string;
}) {
  return (
    <ReactFlowProvider>
      <Chart nodes={props.nodes} edges={props.edges} height={props.height ?? 340} />
    </ReactFlowProvider>
  );
}
