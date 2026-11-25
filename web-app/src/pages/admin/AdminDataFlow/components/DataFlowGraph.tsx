import {
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  MarkerType,
  type Node,
  ReactFlow,
  ReactFlowProvider
} from "@xyflow/react";
import { useEffect, useMemo, useState } from "react";
import "@xyflow/react/dist/style.css";
import { XY_CONTROLS_THEME } from "@/components/airway/LineageGraph";
import { layoutColumns } from "@/components/graph/elkLayout";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import {
  buildLinks,
  columnOf,
  columnPlacement,
  type Flow,
  type FlowLink,
  layoutEdges,
  linkStyle
} from "../dataFlowModel";
import { captionFor, flagsFor, isGroup, type ViewEdge } from "../dataFlowView";
import {
  DataFlowNodeCard,
  type DataFlowNodeData,
  NODE_HEIGHT,
  NODE_WIDTH
} from "./DataFlowNodeCard";
import { FlowEdge, type FlowEdgeData, useReducedMotion } from "./FlowEdge";

const nodeTypes = { dataFlow: DataFlowNodeCard };
const edgeTypes = { flow: FlowEdge };

/** Where each node sits, by id, once ELK has placed them. */
type Positions = Map<string, { x: number; y: number }>;

type Placed = {
  flow: Flow;
  positions: Positions;
  /** ELK refused the graph: why, and the nodes are stacked by column instead. */
  failure?: string;
};

const FALLBACK_BOX = { width: NODE_WIDTH, height: NODE_HEIGHT, gapX: 96, gapY: 20 };

/**
 * ELK runs off the render path; the canvas waits for its answer per graph. A
 * refusal falls back to `columnPlacement` and says so, rather than leaving the
 * canvas on "Laying out…" for good.
 */
function usePositions(flow: Flow, links: FlowLink[]): Omit<Placed, "flow"> | null {
  const [placed, setPlaced] = useState<Placed | null>(null);
  useEffect(() => {
    let live = true;
    const boxes: Node[] = flow.nodes.map((n) => ({
      id: n.id,
      position: { x: 0, y: 0 },
      data: {},
      width: NODE_WIDTH,
      height: NODE_HEIGHT
    }));
    const columns = new Map(flow.nodes.map((n) => [n.id, columnOf(n)] as const));
    layoutColumns(boxes, layoutEdges(links), (n) => columns.get(n.id) ?? 0)
      .then((laid) => {
        if (live) setPlaced({ flow, positions: new Map(laid.map((n) => [n.id, n.position])) });
      })
      .catch((error: unknown) => {
        console.error("data flow layout failed", error);
        if (!live) return;
        setPlaced({
          flow,
          positions: columnPlacement(flow.nodes, FALLBACK_BOX),
          failure: error instanceof Error ? error.message : String(error)
        });
      });
    return () => {
      live = false;
    };
  }, [flow, links]);
  return placed?.flow === flow ? placed : null;
}

function toEdge(link: FlowLink, lit: boolean, dim: boolean, still: boolean): Edge {
  const style = linkStyle(link);
  const count = (link.edge as ViewEdge | undefined)?.count ?? 1;
  return {
    id: link.id,
    type: "flow",
    // A line moves when it sends between systems or the selection lights it, and the
    // viewer has not asked for less motion. A dimmed line never does.
    data: {
      moving: !still && !dim && (style.moving || lit),
      stroke: style.stroke
    } satisfies FlowEdgeData,
    source: link.source,
    target: link.target,
    sourceHandle: link.backward ? "out-l" : "out-r",
    targetHandle: link.backward ? "in-r" : "in-l",
    selectable: false,
    // A folded line stands for several declared edges: say how many, and draw it heavier.
    ...(count > 1 ? { label: `×${count}`, labelStyle: { fontSize: 10 } } : {}),
    style: {
      stroke: style.stroke,
      strokeDasharray: style.dash,
      strokeWidth: (lit ? 2 : 1.25) + (count > 1 ? 0.75 : 0),
      opacity: dim ? 0.15 : 1
    },
    markerEnd: style.arrow
      ? { type: MarkerType.ArrowClosed, color: style.stroke, width: 14, height: 14 }
      : undefined
  };
}

/**
 * The org's flow on a canvas, left to right in fixed columns: sources, pipelines,
 * stores, functions, apps. ELK places the nodes; the lines keep the way the data
 * really runs, so an app writing a store draws right to left. Clicking a node
 * selects it — its lines lit, the rest dimmed; clicking a folded group opens it.
 */
export const DataFlowGraph = ({
  flow,
  focus,
  selected,
  matches,
  onSelect,
  onExpand
}: {
  /** What to draw: the server's answer as the view folded and filtered it. */
  flow: Flow;
  /** The node the server drew the neighbourhood around (`?focus=`). */
  focus: string | null;
  selected: string | null;
  /** The search's matches, outlined. */
  matches: Set<string>;
  onSelect: (id: string | null) => void;
  onExpand: (groupId: string) => void;
}) => {
  const links = useMemo(() => buildLinks(flow), [flow]);
  const drawnKey = useMemo(() => flow.nodes.map((n) => n.id).join("|"), [flow]);
  const placed = usePositions(flow, links);
  const still = useReducedMotion();
  const positions = placed?.positions ?? null;

  const near = useMemo(() => {
    if (!selected) return null;
    const ids = new Set([selected]);
    for (const l of links) {
      if (l.source === selected) ids.add(l.target);
      if (l.target === selected) ids.add(l.source);
    }
    return ids;
  }, [links, selected]);

  const nodes: Node[] = useMemo(
    () =>
      flow.nodes.map((n) => ({
        id: n.id,
        type: "dataFlow",
        position: positions?.get(n.id) ?? { x: 0, y: 0 },
        draggable: false,
        data: {
          node: n,
          caption: captionFor(n),
          flags: flagsFor(n),
          focused: n.id === focus,
          selected: n.id === selected,
          matched: matches.has(n.id),
          count: isGroup(n) ? n.group.members.length : undefined,
          dimmed: near !== null && !near.has(n.id)
        } satisfies DataFlowNodeData
      })),
    [flow, positions, focus, selected, matches, near]
  );

  const edges = useMemo(
    () =>
      links.map((l) => {
        const touches = selected !== null && (l.source === selected || l.target === selected);
        return toEdge(l, touches, selected !== null && !touches, still);
      }),
    [links, selected, still]
  );

  return (
    <div
      className='relative h-160 w-full min-w-0'
      style={XY_CONTROLS_THEME}
      data-testid='admin-data-flow-canvas'
    >
      {placed?.failure && (
        <p
          className={cn("absolute inset-x-3 top-2 z-10 truncate text-xs", ADMIN_TONE.warn.text)}
          title={placed.failure}
          data-testid='admin-data-flow-layout-failed'
        >
          Couldn&rsquo;t lay out this graph ({placed.failure}), so its nodes are stacked by column.
        </p>
      )}
      {positions ? (
        // Keyed by what is drawn, so opening or folding a group fits the new graph to the view.
        <ReactFlowProvider key={drawnKey}>
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            edgeTypes={edgeTypes}
            fitView
            fitViewOptions={{ maxZoom: 1 }}
            minZoom={0.1}
            nodesDraggable={false}
            nodesConnectable={false}
            onNodeClick={(_, node) => {
              const clicked = flow.nodes.find((n) => n.id === node.id);
              if (clicked && isGroup(clicked)) onExpand(clicked.id);
              else onSelect(node.id === selected ? null : node.id);
            }}
            onPaneClick={() => onSelect(null)}
            proOptions={{ hideAttribution: true }}
          >
            <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
            <Controls showInteractive={false} />
          </ReactFlow>
        </ReactFlowProvider>
      ) : (
        <div className='flex h-full items-center justify-center text-muted-foreground text-xs'>
          Laying out {flow.nodes.length} nodes…
        </div>
      )}
    </div>
  );
};
