/**
 * Data-lineage graph for an airway run — the "watch the data move"
 * view, an alternative to the compact `ResourceGrid`.
 *
 * Presentation only — derived from the same `AirwayRunView` the grid
 * uses, so it updates live as the SSE stream folds in. Nodes are
 * tables (source resource → normalized parent → nested child tables),
 * laid out left→right by nesting depth; every root resource flows
 * into a single destination node. No backend involvement.
 */

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
  ReactFlowProvider
} from "@xyflow/react";
import { Database } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import "@xyflow/react/dist/style.css";
import { cn } from "@/libs/shadcn/utils";
import type { AirwayRunView, ResourceRow, ResourceStatus } from "@/utils/airwayReducer";

const COL_W = 240;
const ROW_H = 84;

/** Border/text classes per status — semantic tokens only. Exported for the admin
 *  Data flow canvas, so the two graphs mark a node the same way. */
export const STATUS_CLASSES: Record<ResourceStatus, string> = {
  pending: "border-border text-muted-foreground",
  extracting: "border-primary",
  normalizing: "border-primary",
  loading: "border-primary",
  done: "border-primary bg-primary/10",
  error: "border-destructive text-destructive"
};

/** xyflow's <Controls> ships light-mode defaults; these map its CSS variables to
 *  the shadcn theme tokens so the buttons follow dark/light mode. */
export const XY_CONTROLS_THEME = {
  "--xy-controls-button-background-color": "var(--background)",
  "--xy-controls-button-background-color-hover": "var(--muted)",
  "--xy-controls-button-color": "var(--foreground)",
  "--xy-controls-button-color-hover": "var(--foreground)",
  "--xy-controls-button-border-color": "var(--border)"
} as React.CSSProperties;

const num = (n?: number) => (n == null ? "—" : n.toLocaleString());

/** Leaf label: `orders__checks__selections` → `selections`. */
const leafLabel = (table: string) => {
  const i = table.lastIndexOf("__");
  return i >= 0 ? table.slice(i + 2) : table;
};

type TableNodeData = {
  label: string;
  fullTable: string;
  status: ResourceStatus;
  rowsExtracted?: number;
  rowsNormalized?: number;
  rowsLoaded?: number;
};

const TableNode: React.FC<NodeProps> = ({ data }) => {
  const d = data as TableNodeData;
  return (
    <div
      className={cn(
        "w-52 rounded-md border-[1.5px] bg-background px-3 py-2 shadow-sm",
        STATUS_CLASSES[d.status]
      )}
      title={d.fullTable}
    >
      <Handle type='target' position={Position.Left} className='!bg-muted-foreground' />
      <div className='truncate font-medium text-sm'>{d.label}</div>
      <div className='mt-1 flex justify-between text-[10px] text-muted-foreground tabular-nums'>
        <span>E {num(d.rowsExtracted)}</span>
        <span>N {num(d.rowsNormalized)}</span>
        <span>L {num(d.rowsLoaded)}</span>
      </div>
      <Handle type='source' position={Position.Right} className='!bg-muted-foreground' />
    </div>
  );
};

const DestinationNode: React.FC<NodeProps> = ({ data }) => {
  const d = data as { label: string };
  return (
    <div className='flex w-44 items-center gap-2 rounded-md border-[1.5px] border-primary bg-primary/10 px-3 py-2 shadow-sm'>
      <Handle type='target' position={Position.Left} className='!bg-muted-foreground' />
      <Database className='h-4 w-4 shrink-0' />
      <span className='truncate font-medium text-sm'>{d.label}</span>
    </div>
  );
};

const nodeTypes = { airwayTable: TableNode, airwayDest: DestinationNode };

/** Depth of a row via its parent chain (roots = 0). */
function depthOf(byTable: Map<string, ResourceRow>, row: ResourceRow): number {
  let depth = 0;
  let cur = row.parent;
  while (cur) {
    depth += 1;
    cur = byTable.get(cur)?.parent;
  }
  return depth;
}

function buildGraph(view: AirwayRunView): { nodes: Node[]; edges: Edge[] } {
  const rows = view.resources;
  if (rows.length === 0) return { nodes: [], edges: [] };

  const byTable = new Map(rows.map((r) => [r.table, r] as const));
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // `view.resources` is already ordered parents-first with children
  // immediately after — a stable visual row order to assign `y`.
  rows.forEach((r, i) => {
    nodes.push({
      id: r.table,
      type: "airwayTable",
      position: { x: depthOf(byTable, r) * COL_W, y: i * ROW_H },
      data: {
        label: r.parent ? leafLabel(r.table) : r.table,
        fullTable: r.table,
        status: r.status,
        rowsExtracted: r.rowsExtracted,
        rowsNormalized: r.rowsNormalized,
        rowsLoaded: r.rowsLoaded
      } satisfies TableNodeData,
      draggable: false
    });
    if (r.parent) {
      edges.push({
        id: `${r.parent}->${r.table}`,
        source: r.parent,
        target: r.table,
        style: { stroke: "var(--muted-foreground)", strokeWidth: 1 }
      });
    }
  });

  if (view.destination) {
    const maxDepth = Math.max(...rows.map((r) => depthOf(byTable, r)));
    const roots = rows.filter((r) => !r.parent);
    nodes.push({
      id: "__destination__",
      type: "airwayDest",
      position: {
        x: (maxDepth + 1) * COL_W,
        y: ((rows.length - 1) * ROW_H) / 2
      },
      data: { label: view.destination },
      draggable: false
    });
    for (const root of roots) {
      edges.push({
        id: `${root.table}->dest`,
        source: root.table,
        target: "__destination__",
        animated: root.status === "loading",
        style: { stroke: "var(--muted-foreground)", strokeWidth: 1, opacity: 0.5 }
      });
    }
  }

  return { nodes, edges };
}

const LineageGraph: React.FC<{ view: AirwayRunView }> = ({ view }) => {
  const { nodes, edges } = useMemo(() => buildGraph(view), [view]);

  if (nodes.length === 0) {
    return (
      <div className='px-4 py-10 text-center text-muted-foreground text-sm'>
        No resources yet — waiting for the first extract.
      </div>
    );
  }

  return (
    <div className='h-[420px] w-full' style={XY_CONTROLS_THEME}>
      <ReactFlowProvider>
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          fitView
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable={false}
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
          <Controls showInteractive={false} />
        </ReactFlow>
      </ReactFlowProvider>
    </div>
  );
};

export default LineageGraph;
