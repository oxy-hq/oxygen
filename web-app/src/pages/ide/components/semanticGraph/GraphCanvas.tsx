import {
  Background,
  BackgroundVariant,
  type ColorMode,
  Controls,
  type EdgeTypes,
  type NodeTypes,
  ReactFlow,
  type Edge as RFEdge,
  type Node as RFNode
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";
import useTheme from "@/stores/useTheme";
import { GraphEdge } from "./GraphEdge";

/** Registered under this key so every surface's edges route identically. */
export const GRAPH_EDGE_TYPE = "semantic-graph-edge";

const edgeTypes: EdgeTypes = { [GRAPH_EDGE_TYPE]: GraphEdge };

interface GraphCanvasProps {
  nodes: RFNode[];
  edges: RFEdge[];
  nodeTypes: NodeTypes;
  onNodeClick?: (event: React.MouseEvent, node: RFNode) => void;
  onEdgeClick?: (event: React.MouseEvent, edge: RFEdge) => void;
  /** Clearing the selection by clicking empty canvas. Every semantic graph
   *  should support it — a graph you can select in but not deselect in is the
   *  kind of small inconsistency users feel without being able to name. */
  onPaneClick?: () => void;
  /** Remounts the flow when the graph's shape changes (so `fitView` re-runs). */
  flowKey?: string;
  /** Floor for manual zoom-out. Defaults to the World Model's 0.3; a surface
   *  whose graph is genuinely larger (hundreds of nodes) may lower it. */
  minZoom?: number;
  /** Overlays drawn above the canvas (filter pills, toggles, legends). */
  children?: ReactNode;
}

/**
 * The shared canvas every IDE semantic graph is drawn on: dotted background,
 * themed controls, read-only panning/zooming, and the reflow animation that
 * makes a layout change read as nodes making room rather than teleporting.
 *
 * Nodes are ELK-positioned, so dragging is disabled everywhere — a dragged node
 * fights the next layout pass and desyncs the ELK-routed edges.
 */
export function GraphCanvas({
  nodes,
  edges,
  nodeTypes,
  onNodeClick,
  onEdgeClick,
  onPaneClick,
  flowKey,
  minZoom = 0.3,
  children
}: GraphCanvasProps) {
  const theme = useTheme((s) => s.theme);

  return (
    <div className='semantic-graph relative h-full w-full'>
      {/* Animate nodes sliding to their new slots when a size-aware reflow moves
          them, so the change reads as the neighbors making room rather than a
          jump. Scoped to this canvas. */}
      <style>{`.semantic-graph .react-flow__node { transition: transform 300ms cubic-bezier(0.22, 0.61, 0.36, 1); }`}</style>
      {children}
      <ReactFlow
        key={flowKey}
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        forceColorMode={theme as ColorMode}
        onNodeClick={onNodeClick}
        onEdgeClick={onEdgeClick}
        onPaneClick={onPaneClick}
        nodesDraggable={false}
        nodesConnectable={false}
        fitView
        fitViewOptions={{ padding: 0.16, minZoom }}
        minZoom={minZoom}
        maxZoom={1.8}
        proOptions={{ hideAttribution: true }}
        style={{ background: "var(--background)" }}
      >
        <Background variant={BackgroundVariant.Dots} color='var(--border)' gap={22} size={1} />
        <Controls
          showInteractive={false}
          className={cn(
            "!overflow-hidden !border !border-border-subtle !bg-card !shadow-none",
            "[&_button]:!border-border [&_button]:!bg-card [&_button]:!fill-foreground",
            "[&_button:hover]:!bg-muted"
          )}
        />
      </ReactFlow>
    </div>
  );
}
