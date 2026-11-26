import {
  Background,
  BackgroundVariant,
  type ColorMode,
  Handle,
  type NodeProps,
  Position,
  ReactFlow,
  type Edge as RFEdge,
  type Node as RFNode
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import ELK from "elkjs";
import { useEffect, useMemo, useState } from "react";
import { splitLabel } from "@/pages/ide/MetricTree/components/ExplainTree";
import useTheme from "@/stores/useTheme";
import type { ExplainNode, ExplainResult } from "@/types/metricTree";

const NODE_WIDTH = 220;
const NODE_HEIGHT = 76;
const elk = new ELK();

interface DistributionNodeData {
  kind: "target" | "split";
  title: string;
  subtitle: string;
  detail: string;
  /** 0-1, drives background opacity so big contributors stand out. */
  intensity: number;
}

function DistributionNode({ data }: NodeProps) {
  const { kind, title, subtitle, detail, intensity } = data as unknown as DistributionNodeData;

  // Use a single semantic token (`--primary`) for split-node tints — the
  // intensity is encoded as opacity via color-mix so big contributors stand
  // out without hardcoding a hex value.
  const splitAlpha = 0.08 + 0.22 * intensity;
  const bgStyle =
    kind === "target"
      ? { backgroundColor: "var(--card)" }
      : {
          backgroundColor: `color-mix(in oklab, var(--primary) ${(splitAlpha * 100).toFixed(1)}%, transparent)`
        };

  return (
    <>
      <Handle type='target' position={Position.Top} className='opacity-0' />
      <div
        className='flex flex-col gap-0.5 border border-border px-3 py-2'
        style={{ width: NODE_WIDTH, minHeight: NODE_HEIGHT, ...bgStyle }}
      >
        <p className='truncate font-medium text-foreground text-sm' title={title}>
          {title}
        </p>
        <p className='truncate text-muted-foreground text-xs' title={subtitle}>
          {subtitle}
        </p>
        <p className='truncate text-foreground/80 text-xs tabular-nums' title={detail}>
          {detail}
        </p>
      </div>
      <Handle type='source' position={Position.Bottom} className='opacity-0' />
    </>
  );
}

const nodeTypes = { "distribution-node": DistributionNode };

interface FlowGraph {
  nodes: RFNode[];
  edges: RFEdge[];
}

function buildGraph(result: ExplainResult): FlowGraph {
  const nodes: RFNode[] = [];
  const edges: RFEdge[] = [];

  const rootId = "root";
  nodes.push({
    id: rootId,
    type: "distribution-node",
    position: { x: 0, y: 0 },
    data: {
      kind: "target",
      title: result.target,
      subtitle: "current value",
      detail: formatNumber(result.target_current),
      intensity: 1
    } satisfies DistributionNodeData,
    width: NODE_WIDTH,
    height: NODE_HEIGHT
  });

  for (let i = 0; i < result.nodes.length; i++) {
    walk(result.nodes[i], rootId, `s${i}`, nodes, edges);
  }

  return { nodes, edges };
}

function walk(
  node: ExplainNode,
  parentId: string,
  path: string,
  nodes: RFNode[],
  edges: RFEdge[]
): void {
  const id = path;
  const sharePct = node.root_fraction * 100;
  nodes.push({
    id,
    type: "distribution-node",
    position: { x: 0, y: 0 },
    data: {
      kind: "split",
      title: splitLabel(node.split),
      subtitle: node.measure,
      detail: `${sharePct.toFixed(1)}% of root`,
      intensity: clampIntensity(Math.abs(node.root_fraction))
    } satisfies DistributionNodeData,
    width: NODE_WIDTH,
    height: NODE_HEIGHT
  });
  edges.push({
    id: `${id}->${parentId}`,
    source: parentId,
    target: id,
    type: "default",
    label: `${sharePct.toFixed(1)}%`,
    labelStyle: {
      fill: "var(--foreground)",
      fontSize: 11,
      fontVariantNumeric: "tabular-nums",
      fontWeight: 500
    },
    labelBgStyle: {
      fill: "var(--background)",
      fillOpacity: 0.85
    },
    labelBgPadding: [6, 2],
    labelBgBorderRadius: 4,
    style: {
      stroke: "var(--muted-foreground)",
      strokeWidth: 1.5
    }
  });

  if (node.children) {
    for (let i = 0; i < node.children.length; i++) {
      walk(node.children[i], id, `${path}.c${i}`, nodes, edges);
    }
  }
}

async function layoutWithElk(nodes: RFNode[], edges: RFEdge[]): Promise<RFNode[]> {
  if (nodes.length === 0) return nodes;
  const graph = {
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "DOWN",
      "elk.layered.spacing.nodeNodeBetweenLayers": "60",
      "elk.spacing.nodeNode": "32"
    },
    children: nodes.map((n) => ({ id: n.id, width: NODE_WIDTH, height: NODE_HEIGHT })),
    edges: edges.map((e) => ({ id: e.id, sources: [e.source], targets: [e.target] }))
  };
  const laid = await elk.layout(graph);
  const positions = new Map((laid.children ?? []).map((c) => [c.id, c]));
  return nodes.map((n) => {
    const pos = positions.get(n.id);
    return pos ? { ...n, position: { x: pos.x ?? 0, y: pos.y ?? 0 } } : n;
  });
}

interface DistributionGraphProps {
  result: ExplainResult;
  /** Fixed pixel height, or `"fill"` to stretch to the parent's height. */
  height?: number | "fill";
}

/** Top-down tree of the selected measure's structural decomposition. Reads
 *  the structure from an `ExplainResult` (so the backend's component-split
 *  search drives the tree), but renders only the current value at the root
 *  and each node's share of root — no period-over-period delta framing. */
export default function DistributionGraph({ result, height = 360 }: DistributionGraphProps) {
  const theme = useTheme((s) => s.theme);
  const { nodes: rawNodes, edges } = useMemo(() => buildGraph(result), [result]);
  const [positioned, setPositioned] = useState<RFNode[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPositioned(null);
    layoutWithElk(rawNodes, edges)
      .then((laidOut) => {
        if (!cancelled) setPositioned(laidOut);
      })
      .catch((error) => {
        console.error("distribution graph layout failed", error);
        if (!cancelled) setPositioned(rawNodes);
      });
    return () => {
      cancelled = true;
    };
  }, [rawNodes, edges]);

  return (
    <div style={{ width: "100%", height: height === "fill" ? "100%" : height }}>
      {positioned === null ? (
        <div className='flex h-full items-center justify-center text-muted-foreground text-xs'>
          Laying out…
        </div>
      ) : (
        <ReactFlow
          key={result.target + result.target_current}
          nodes={positioned}
          edges={edges}
          nodeTypes={nodeTypes}
          forceColorMode={theme as ColorMode}
          fitView
          fitViewOptions={{ padding: 0.1 }}
          minZoom={0.05}
          proOptions={{ hideAttribution: true }}
          nodesDraggable={false}
          nodesConnectable={false}
          edgesFocusable={false}
        >
          <Background variant={BackgroundVariant.Dots} gap={16} />
        </ReactFlow>
      )}
    </div>
  );
}

function clampIntensity(n: number): number {
  if (!Number.isFinite(n)) return 0;
  return Math.max(0, Math.min(1, n));
}

function formatNumber(n: number): string {
  if (Math.abs(n) >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (Math.abs(n) >= 1_000) return `${(n / 1_000).toFixed(2)}k`;
  return n.toFixed(2);
}
