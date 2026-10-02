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
import ELK from "elkjs";
import { CheckCircle2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import type { BuilderFileChange } from "@/hooks/useBuilderActivity";
import { cn } from "@/libs/shadcn/utils";
import useTheme from "@/stores/useTheme";
import { elkEdgeTypes } from "./ElkRoutedEdge";
import { elkSectionToSvgPath, HANDLE_STYLE, makeEdge, NODE_W } from "./graphUtils";
import { diffFields, type FieldDiff, findRelatedDims, type SemanticView } from "./types";

const elk = new ELK();

// ── Node components ───────────────────────────────────────────────────────────

const RfViewNode = ({ data }: NodeProps) => {
  const d = data as {
    view: SemanticView;
    dimCount: number;
    measureCount: number;
    addedCount: number;
    modifiedCount: number;
    removedCount: number;
  };
  return (
    <>
      <Handle type='source' position={Position.Right} id='right' style={HANDLE_STYLE} />
      <Handle type='source' position={Position.Bottom} id='bottom' style={HANDLE_STYLE} />
      <div className='flex min-h-[80px] flex-col justify-center rounded-lg border-2 border-primary/50 bg-card px-4 py-3 shadow-sm'>
        <p className='font-mono text-[10px] text-primary/60 uppercase tracking-wider'>View</p>
        <p className='font-bold text-foreground text-sm'>{d.view.name}</p>
        <p className='mt-0.5 text-[11px] text-muted-foreground'>
          {d.dimCount} dims · {d.measureCount} measures
          {d.addedCount > 0 && <span className='ml-1 text-emerald-500'>· +{d.addedCount}</span>}
          {d.modifiedCount > 0 && <span className='ml-1 text-amber-500'>· ~{d.modifiedCount}</span>}
          {d.removedCount > 0 && <span className='ml-1 text-destructive'>· -{d.removedCount}</span>}
        </p>
      </div>
    </>
  );
};

const RfSourceNode = ({ data }: NodeProps) => {
  const d = data as { datasource?: string; table?: string };
  return (
    <>
      <Handle type='target' position={Position.Left} id='left' style={HANDLE_STYLE} />
      <div className='flex min-h-[80px] flex-col justify-center rounded-lg border border-border bg-card px-4 py-3 shadow-sm'>
        <p className='font-mono text-[10px] text-muted-foreground/60 uppercase tracking-wider'>
          Source
        </p>
        <p className='font-bold text-foreground text-sm'>{d.datasource ?? "datasource"}</p>
        {d.table && <p className='font-mono text-[10px] text-muted-foreground'>.{d.table}</p>}
      </div>
    </>
  );
};

const RfChangedFieldNode = ({ data }: NodeProps) => {
  const { diff } = data as { diff: FieldDiff };
  const isAdded = diff.status === "added";
  const isModified = diff.status === "modified";
  const isRemoved = diff.status === "removed";
  const label = diff.kind === "measure" ? "Measure" : "Dim";
  const prefix = isAdded ? "+ " : isRemoved ? "- " : "";
  return (
    <>
      <Handle type='target' position={Position.Top} id='top' style={HANDLE_STYLE} />
      <Handle type='source' position={Position.Bottom} id='bottom' style={HANDLE_STYLE} />
      <div
        className={cn(
          "rounded-lg border-2 px-4 py-3 shadow-sm",
          isAdded
            ? "border-emerald-500/40 bg-card"
            : isModified
              ? "border-amber-500/40 bg-card"
              : "border-destructive/40 bg-card"
        )}
      >
        <div className='mb-1 flex items-center gap-1.5'>
          <span
            className={cn(
              "h-2 w-2 rounded-full",
              isAdded ? "bg-emerald-500" : isModified ? "bg-amber-500" : "bg-destructive"
            )}
          />
          <span
            className={cn(
              "font-mono text-[10px] uppercase tracking-wider",
              isAdded ? "text-emerald-500" : isModified ? "text-amber-500" : "text-destructive"
            )}
          >
            {prefix}
            {label}
          </span>
        </div>
        <p className='truncate font-bold text-foreground text-sm'>{diff.name}</p>
        {diff.field.type && (
          <p className='mt-0.5 text-[11px] text-muted-foreground'>
            type: <span className='font-semibold text-foreground/80'>{diff.field.type}</span>
          </p>
        )}
        {diff.field.expr && (
          <p className='truncate font-mono text-[10px] text-muted-foreground'>
            sql: {diff.field.expr}
          </p>
        )}
        {diff.changes && (
          <div className='mt-1.5 space-y-0.5'>
            {diff.changes.map((c) => (
              <p key={c} className='font-mono text-[10px] text-amber-500'>
                {c}
              </p>
            ))}
          </div>
        )}
      </div>
    </>
  );
};

const RfRelatedDimNode = ({ data }: NodeProps) => {
  const { name } = data as { name: string };
  return (
    <>
      <Handle type='target' position={Position.Top} id='top' style={HANDLE_STYLE} />
      <div className='rounded-lg border border-border border-dashed bg-card px-3 py-2 shadow-sm'>
        <p className='font-mono text-[10px] text-muted-foreground/60 uppercase tracking-wider'>
          Dim
        </p>
        <p className='font-medium text-muted-foreground text-xs'>{name}</p>
      </div>
    </>
  );
};

// nodeTypes must be stable (defined outside components)
const nodeTypes = {
  view: RfViewNode,
  source: RfSourceNode,
  changedField: RfChangedFieldNode,
  relatedDim: RfRelatedDimNode
};

// ── SemanticViewGraph ─────────────────────────────────────────────────────────

export const SemanticViewGraph = ({
  change,
  oldView,
  newView
}: {
  change: BuilderFileChange;
  oldView: SemanticView | null;
  newView: SemanticView;
}) => {
  const { theme } = useTheme();

  const dimDiffs = useMemo(
    () => diffFields(oldView?.dimensions ?? [], newView.dimensions ?? [], "dim"),
    [oldView, newView]
  );
  const measureDiffs = useMemo(
    () => diffFields(oldView?.measures ?? [], newView.measures ?? [], "measure"),
    [oldView, newView]
  );
  const changedFields = useMemo(
    () => [...dimDiffs, ...measureDiffs].filter((d) => d.status !== "unchanged"),
    [dimDiffs, measureDiffs]
  );

  const addedCount = changedFields.filter((d) => d.status === "added").length;
  const modifiedCount = changedFields.filter((d) => d.status === "modified").length;
  const removedCount = changedFields.filter((d) => d.status === "removed").length;
  const dimCount = (newView.dimensions ?? []).length;
  const measureCount = (newView.measures ?? []).length;

  const allDims = newView.dimensions ?? [];
  const relatedDimNames = useMemo(() => {
    const names = new Set<string>();
    for (const cf of changedFields) {
      if (cf.kind !== "dim") continue;
      for (const dn of findRelatedDims(cf.field, allDims)) {
        if (!changedFields.some((c) => c.name === dn)) names.add(dn);
      }
    }
    return Array.from(names);
  }, [changedFields, allDims]);

  const changedToRelated = useMemo(() => {
    const map: Map<number, number[]> = new Map();
    changedFields.forEach((cf, ci) => {
      if (cf.kind !== "dim") return;
      const related = findRelatedDims(cf.field, allDims)
        .filter((dn) => relatedDimNames.includes(dn))
        .map((dn) => relatedDimNames.indexOf(dn))
        .filter((i) => i >= 0);
      if (related.length > 0) map.set(ci, related);
    });
    return map;
  }, [changedFields, relatedDimNames, allDims]);

  const hasSource = !!(newView.datasource || newView.table);

  const baseNodes = useMemo<RFNode[]>(() => {
    const nodes: RFNode[] = [];
    nodes.push({
      id: "view",
      type: "view",
      position: { x: 0, y: 0 },
      width: NODE_W,
      data: { view: newView, dimCount, measureCount, addedCount, modifiedCount, removedCount }
    });
    if (hasSource) {
      nodes.push({
        id: "source",
        type: "source",
        position: { x: 0, y: 0 },
        width: NODE_W,
        data: { datasource: newView.datasource, table: newView.table }
      });
    }
    changedFields.forEach((diff, i) => {
      nodes.push({
        id: `changed-${i}`,
        type: "changedField",
        position: { x: 0, y: 0 },
        width: NODE_W,
        data: { diff }
      });
    });
    relatedDimNames.forEach((name, i) => {
      nodes.push({
        id: `related-${i}`,
        type: "relatedDim",
        position: { x: 0, y: 0 },
        width: NODE_W,
        data: { name }
      });
    });
    return nodes;
  }, [
    newView,
    dimCount,
    measureCount,
    addedCount,
    modifiedCount,
    removedCount,
    hasSource,
    changedFields,
    relatedDimNames
  ]);

  const rfEdges = useMemo<RFEdge[]>(() => {
    const edges: RFEdge[] = [];
    if (hasSource) {
      edges.push(
        makeEdge("view-source", "view", "source", {
          sourceHandle: "right",
          targetHandle: "left",
          straight: true
        })
      );
    }
    changedFields.forEach((diff, i) => {
      edges.push(
        makeEdge(`view-changed-${i}`, "view", `changed-${i}`, {
          sourceHandle: "bottom",
          targetHandle: "top",
          status: diff.status
        })
      );
    });
    changedToRelated.forEach((relIdxs, ci) => {
      for (const ri of relIdxs) {
        edges.push(
          makeEdge(`changed-${ci}-rel-${ri}`, `changed-${ci}`, `related-${ri}`, {
            sourceHandle: "bottom",
            targetHandle: "top",
            dashed: true
          })
        );
      }
    });
    return edges;
  }, [hasSource, changedFields, changedToRelated]);

  const [layoutedNodes, setLayoutedNodes] = useState<RFNode[]>([]);
  const [layoutedEdges, setLayoutedEdges] = useState<RFEdge[]>([]);
  const [rfInstance, setRfInstance] = useState<{ fitView: (opts?: object) => void } | null>(null);
  const [layoutFailed, setLayoutFailed] = useState(false);

  useEffect(() => {
    if (baseNodes.length === 0) return;

    // Layout is async and the inputs change while the builder streams: a slower,
    // earlier layout must not overwrite the result of a newer one.
    let cancelled = false;

    // Source is excluded from ELK so it stays on the same row as view (to its right).
    const elkNodes = baseNodes
      .filter((node) => node.id !== "source")
      .map((node) => {
        let height = 80;
        if (node.type === "changedField") {
          const diff = (node.data as { diff: FieldDiff }).diff;
          height = 100 + (diff.changes?.length ?? 0) * 18;
        } else if (node.type === "relatedDim") {
          height = 60;
        }
        return { id: node.id, width: NODE_W, height };
      });

    const elkEdges = rfEdges
      .filter((e) => e.source !== "source" && e.target !== "source")
      .map((e) => ({ id: e.id, sources: [e.source], targets: [e.target] }));

    elk
      .layout({
        id: "semantic-view-graph",
        layoutOptions: {
          "elk.algorithm": "layered",
          "elk.direction": "DOWN",
          "elk.layered.layering.strategy": "COFFMAN_GRAHAM",
          "elk.layered.layering.coffmanGraham.layerBound": "3",
          "elk.spacing.nodeNode": "16",
          "elk.layered.spacing.nodeNodeBetweenLayers": "60"
        },
        children: elkNodes,
        edges: elkEdges
      })
      .then((layout) => {
        if (cancelled) return;
        setLayoutFailed(false);
        const viewPos = layout.children?.find((n) => n.id === "view");
        setLayoutedNodes(
          baseNodes.map((node) => {
            if (node.id === "source") {
              return {
                ...node,
                position: { x: (viewPos?.x ?? 0) + NODE_W + 32, y: viewPos?.y ?? 0 }
              };
            }
            const pos = layout.children?.find((n) => n.id === node.id);
            return { ...node, position: { x: pos?.x ?? 0, y: pos?.y ?? 0 } };
          })
        );

        const pathMap = new Map<string, string>();
        for (const e of layout.edges ?? []) {
          const section = (
            e as {
              sections?: {
                startPoint: { x: number; y: number };
                endPoint: { x: number; y: number };
                bendPoints?: { x: number; y: number }[];
              }[];
            }
          ).sections?.[0];
          if (section) pathMap.set(e.id, elkSectionToSvgPath(section));
        }
        setLayoutedEdges(
          rfEdges.map((e) => {
            const svgPath = pathMap.get(e.id);
            return svgPath ? { ...e, type: "elkRouted", data: { svgPath } } : e;
          })
        );
      })
      // ELK rejects on a graph it cannot lay out (e.g. an edge naming a node that
      // is not in `children`). Left alone the canvas stays blank, or keeps showing the
      // graph of the previous inputs — clear it and say so instead.
      .catch((error: unknown) => {
        console.error("Semantic view graph layout failed", error);
        if (cancelled) return;
        setLayoutedNodes([]);
        setLayoutedEdges([]);
        setLayoutFailed(true);
      });

    return () => {
      cancelled = true;
    };
  }, [baseNodes, rfEdges]);

  useEffect(() => {
    if (layoutedNodes.length === 0 || !rfInstance) return;
    // Double rAF: first frame lets React commit the new positions, second
    // lets ReactFlow measure nodes before fitView calculates the bounding box.
    let raf1: number;
    let raf2: number;
    raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => {
        rfInstance.fitView({ padding: 0.15, maxZoom: 0.8 });
      });
    });
    return () => {
      cancelAnimationFrame(raf1);
      cancelAnimationFrame(raf2);
    };
  }, [layoutedNodes, rfInstance]);

  return (
    <div className='flex h-full flex-col gap-3'>
      <div className='shrink-0 space-y-1'>
        <div className='flex items-center gap-2'>
          <span className='font-mono text-[10px] text-muted-foreground/60 uppercase tracking-widest'>
            Context Graph
          </span>
          {change.status === "accepted" && (
            <span className='flex items-center gap-0.5 rounded bg-emerald-500/15 px-1.5 py-0.5 font-bold text-[10px] text-emerald-600 uppercase tracking-wide dark:text-emerald-400'>
              <CheckCircle2 className='h-2.5 w-2.5' /> Accepted
            </span>
          )}
          {change.status === "rejected" && (
            <span className='flex items-center gap-0.5 rounded bg-destructive/15 px-1.5 py-0.5 font-bold text-[10px] text-destructive uppercase tracking-wide'>
              <X className='h-2.5 w-2.5' /> Skipped
            </span>
          )}
          {change.status === "pending" && changedFields.length > 0 && (
            <span className='rounded bg-emerald-500/15 px-1.5 py-0.5 font-bold text-[10px] text-emerald-600 dark:text-emerald-400'>
              +{changedFields.length} pending
            </span>
          )}
        </div>
        {change.description && (
          <p className='text-muted-foreground text-xs'>{change.description}</p>
        )}
      </div>

      <div className='relative min-h-0 flex-1'>
        <ReactFlow
          nodes={layoutedNodes}
          edges={layoutedEdges}
          nodeTypes={nodeTypes}
          edgeTypes={elkEdgeTypes}
          colorMode={theme as ColorMode}
          onInit={setRfInstance}
          minZoom={0.1}
          defaultViewport={{ x: 0, y: 0, zoom: 1 }}
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable={false}
          panOnDrag={true}
          zoomOnScroll={true}
          zoomOnPinch={true}
          zoomOnDoubleClick={false}
          preventScrolling={false}
          proOptions={{ hideAttribution: true }}
        >
          <Background
            color={theme === "dark" ? "#a9a9b2" : "#ddd"}
            bgColor={theme === "dark" ? "oklch(14.5% 0 0)" : "oklch(1 0 0)"}
            variant={BackgroundVariant.Dots}
          />
        </ReactFlow>
        {layoutFailed && (
          <div className='pointer-events-none absolute inset-0 flex items-center justify-center text-destructive text-sm'>
            Failed to lay out this graph
          </div>
        )}
      </div>
    </div>
  );
};
