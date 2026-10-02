import {
  Background,
  BackgroundVariant,
  type ColorMode,
  Handle,
  type NodeProps,
  Position,
  ReactFlow,
  type ReactFlowInstance,
  type Edge as RFEdge,
  type Node as RFNode
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import ELK from "elkjs";
import { useEffect, useMemo, useRef, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { splitLabel } from "@/pages/ide/MetricTree/components/ExplainTree";
import useTheme from "@/stores/useTheme";
import type { ExplainNode, ExplainResult } from "@/types/metricTree";
import { formatNumber, formatSigned } from "@/utils/measureFormat";
import { driverPush, roleBadge } from "./driverClassification";

const NODE_WIDTH = 220;
const NODE_HEIGHT = 84;
/** Weight for a driver with no `estimated_target_impact`. There is no honest
 *  ratio to compute for one (see the call site), so every qualitative driver
 *  renders at the same middling emphasis: present, but not ranked. */
const QUALITATIVE_INTENSITY = 0.35;
const elk = new ELK();

// ── Node payload ────────────────────────────────────────────────────────────

type ExplainNodeKind = "target" | "split" | "driver";

interface ExplainGraphNodeData {
  kind: ExplainNodeKind;
  title: string;
  subtitle: string;
  detail: string;
  /** 0-1, used for color intensity. */
  intensity: number;
  /** Positive deltas get the success tone, negative the destructive tone,
   *  zero/unknown stays neutral. */
  direction: "up" | "down" | "neutral";
}

// ── Node renderer ───────────────────────────────────────────────────────────

function GraphNode({ data }: NodeProps) {
  const { kind, title, subtitle, detail, intensity, direction } =
    data as unknown as ExplainGraphNodeData;

  const directionClass =
    direction === "up"
      ? "border-success/50"
      : direction === "down"
        ? "border-destructive/50"
        : "border-border";

  // Map intensity (0-1) → background opacity so the eye picks out big
  // contributors immediately. Target node gets a neutral solid bg. We mix
  // the direction tone with `transparent` via `color-mix` so the alpha
  // ramp lives in CSS rather than hardcoded rgba values.
  const tintPct = ((0.08 + 0.22 * intensity) * 100).toFixed(1);
  const bgStyle =
    kind === "target"
      ? { backgroundColor: "var(--card)" }
      : direction === "up"
        ? {
            backgroundColor: `color-mix(in oklab, var(--success) ${tintPct}%, transparent)`
          }
        : direction === "down"
          ? {
              backgroundColor: `color-mix(in oklab, var(--destructive) ${tintPct}%, transparent)`
            }
          : { backgroundColor: "var(--muted)" };

  return (
    <>
      <Handle type='target' position={Position.Top} className='opacity-0' />
      <div
        className={cn(
          "flex flex-col gap-0.5 rounded-lg border px-3 py-2 shadow-sm transition-colors",
          directionClass
        )}
        style={{ width: NODE_WIDTH, minHeight: NODE_HEIGHT, ...bgStyle }}
      >
        <p className='truncate font-medium text-foreground text-sm'>{title}</p>
        <p className='truncate text-muted-foreground text-xs'>{subtitle}</p>
        <p className='truncate text-foreground/80 text-xs tabular-nums'>{detail}</p>
      </div>
      <Handle type='source' position={Position.Bottom} className='opacity-0' />
    </>
  );
}

const nodeTypes = { "explain-node": GraphNode };

// ── Tree → React Flow conversion ────────────────────────────────────────────

interface FlowGraph {
  nodes: RFNode[];
  edges: RFEdge[];
}

/** Convert an [`ExplainResult`] into a React Flow graph. The target measure
 *  is the root; each top-level split (and its children) becomes a downstream
 *  branch. Driver attributions hang off the root in their own branch.
 *
 *  Node ids are stable strings derived from a path so React Flow can diff
 *  cleanly across re-renders. */
function explainToFlow(result: ExplainResult): FlowGraph {
  const nodes: RFNode[] = [];
  const edges: RFEdge[] = [];

  const rootId = "root";
  const direction = numberDirection(result.target_delta);
  nodes.push({
    id: rootId,
    type: "explain-node",
    position: { x: 0, y: 0 },
    data: {
      kind: "target",
      title: result.target,
      subtitle: `${result.nodes.length} split${result.nodes.length !== 1 ? "s" : ""} found`,
      detail: `${formatNumber(result.target_previous)} → ${formatNumber(result.target_current)} (${
        direction === "up" ? "+" : ""
      }${formatNumber(result.target_delta)})`,
      intensity: 1,
      direction
    } satisfies ExplainGraphNodeData,
    width: NODE_WIDTH,
    height: NODE_HEIGHT
  });

  // Walk each top-level decomposition node.
  for (let i = 0; i < result.nodes.length; i++) {
    walk(result.nodes[i], rootId, `s${i}`, result.target_delta, nodes, edges);
  }

  // Driver attributions as a sibling branch off root.
  if (result.driver_attribution) {
    for (let i = 0; i < result.driver_attribution.length; i++) {
      const driver = result.driver_attribution[i];
      const id = `driver-${i}`;
      const impact = driver.estimated_target_impact;
      const quantified = impact !== undefined && impact !== null;
      // Tone by the driver's push *on the target*, never by the sign of its own
      // delta — see `driverPush`. `null` means no signed claim, which must stay
      // neutral rather than defaulting into the target's own tone.
      const push = driverPush(driver, result.target_delta);
      // `intensity` reads as "how much of the target's move this accounts for",
      // so it is only computable from `estimated_target_impact` — the one figure
      // in the *target's* units. Substituting `driver_delta` divides a
      // driver-unit quantity by a target-unit one: a rate driver (Δ 0.0038
      // against a target Δ of -589) lands at ~0 anyway, and a count-scale driver
      // saturates at 1.0 while claiming a certainty it has not earned. So a
      // qualitative driver gets a fixed middling weight instead — visible,
      // ranked against nothing, with `roleBadge` carrying the meaning.
      nodes.push({
        id,
        type: "explain-node",
        position: { x: 0, y: 0 },
        data: {
          kind: "driver",
          title: `driver · ${driver.driver_measure}`,
          subtitle: quantified
            ? `est. impact ${formatSigned(impact)} · ${roleBadge(driver)}`
            : `qualitative · ${roleBadge(driver)}`,
          detail: `Δ ${formatSigned(driver.driver_delta)}`,
          intensity: quantified
            ? clampIntensity(Math.abs(impact) / Math.max(Math.abs(result.target_delta), 1e-9))
            : QUALITATIVE_INTENSITY,
          direction: push === null ? "neutral" : numberDirection(push)
        } satisfies ExplainGraphNodeData,
        width: NODE_WIDTH,
        height: NODE_HEIGHT
      });
      edges.push(edge(id, rootId, "driver"));
    }
  }

  return { nodes, edges };
}

/** Recursive walk: emit one node per ExplainNode, link to parent, recurse
 *  into its children. */
function walk(
  node: ExplainNode,
  parentId: string,
  path: string,
  parentDelta: number,
  nodes: RFNode[],
  edges: RFEdge[]
): void {
  const id = path;
  const direction = numberDirection(node.delta);
  const intensity = clampIntensity(Math.abs(node.root_fraction));
  nodes.push({
    id,
    type: "explain-node",
    position: { x: 0, y: 0 },
    data: {
      kind: "split",
      title: splitLabel(node.split),
      subtitle: node.measure,
      detail: `Δ ${direction === "up" ? "+" : ""}${formatNumber(node.delta)}`,
      intensity,
      direction
    } satisfies ExplainGraphNodeData,
    width: NODE_WIDTH,
    height: NODE_HEIGHT
  });
  const pct =
    Math.abs(parentDelta) > 1e-9 ? `${((node.delta / parentDelta) * 100).toFixed(1)}%` : undefined;
  edges.push(edge(id, parentId, "split", pct));

  if (node.children) {
    for (let i = 0; i < node.children.length; i++) {
      walk(node.children[i], id, `${path}.c${i}`, node.delta, nodes, edges);
    }
  }
}

function edge(
  sourceId: string,
  targetId: string,
  kind: "split" | "driver",
  label?: string
): RFEdge {
  const isDriver = kind === "driver";
  return {
    id: `${sourceId}->${targetId}`,
    source: targetId,
    target: sourceId,
    type: "default",
    animated: isDriver,
    label,
    labelStyle: { fill: "var(--foreground)", fontSize: 11, fontWeight: 500 },
    labelBgStyle: { fill: "var(--background)", fillOpacity: 0.85 },
    labelBgPadding: [6, 2],
    labelBgBorderRadius: 4,
    style: {
      stroke: isDriver ? "var(--primary)" : "var(--muted-foreground)",
      strokeWidth: 1.5,
      strokeDasharray: isDriver ? "6 4" : undefined
    }
  };
}

// ── Helpers ─────────────────────────────────────────────────────────────────

function numberDirection(n: number): "up" | "down" | "neutral" {
  if (n > 1e-9) return "up";
  if (n < -1e-9) return "down";
  return "neutral";
}

function clampIntensity(n: number): number {
  if (!Number.isFinite(n)) return 0;
  return Math.max(0, Math.min(1, n));
}

/** Position nodes top-down with ELK. Mirrors `MetricTreeGraph`'s layout. */
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
  const result = await elk.layout(graph);
  const positions = new Map((result.children ?? []).map((c) => [c.id, c]));
  return nodes.map((n) => {
    const pos = positions.get(n.id);
    return pos ? { ...n, position: { x: pos.x ?? 0, y: pos.y ?? 0 } } : n;
  });
}

// ── Public component ────────────────────────────────────────────────────────

interface ExplainGraphProps {
  result: ExplainResult;
  /** Fixed pixel height, or `"fill"` to stretch to the parent's height. */
  height?: number | "fill";
}

/** Render the explain decomposition as an interactive top-down tree.
 *
 *  Target measure at the root, splits as downstream branches, driver
 *  attributions on the side. Background intensity encodes contribution
 *  size; green = positive delta, red = negative delta. */
export default function ExplainGraph({ result, height = 360 }: ExplainGraphProps) {
  const theme = useTheme((s) => s.theme);
  const { nodes: rawNodes, edges } = useMemo(() => explainToFlow(result), [result]);
  const [positioned, setPositioned] = useState<RFNode[] | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const rfRef = useRef<ReactFlowInstance | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPositioned(null);
    layoutWithElk(rawNodes, edges)
      .then((laidOut) => {
        if (!cancelled) setPositioned(laidOut);
      })
      .catch((error) => {
        console.error("explain graph layout failed", error);
        if (!cancelled) setPositioned(rawNodes);
      });
    return () => {
      cancelled = true;
    };
  }, [rawNodes, edges]);

  // Re-run fitView when the container transitions from 0 → positive size.
  // This happens in animated containers (Sheet slide-in, tab switches) where
  // the container has 0 dimensions when ReactFlow first mounts and calls
  // fitView, then grows to its real size — but ReactFlow doesn't auto-refit
  // on container resize, so nodes end up invisible at the wrong viewport position.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    let hadSize = false;
    const ro = new ResizeObserver(([entry]) => {
      const { width, height: h } = entry.contentRect;
      const hasSize = width > 0 && h > 0;
      if (hasSize && !hadSize) {
        // React Flow's `fitView` only ever resolves (to whether it fitted).
        void rfRef.current?.fitView({ padding: 0.2 });
      }
      hadSize = hasSize;
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const isFill = height === "fill";
  return (
    <div
      ref={containerRef}
      className={isFill ? "absolute inset-0" : ""}
      style={isFill ? undefined : { width: "100%", height }}
    >
      {positioned === null ? (
        <div className='flex h-full items-center justify-center text-muted-foreground text-xs'>
          Laying out…
        </div>
      ) : (
        <ReactFlow
          key={result.target + result.target_delta}
          nodes={positioned}
          edges={edges}
          nodeTypes={nodeTypes}
          colorMode={theme as ColorMode}
          onInit={(instance) => {
            rfRef.current = instance;
          }}
          fitView
          fitViewOptions={{ padding: 0.2, minZoom: 0.1 }}
          minZoom={0.1}
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
