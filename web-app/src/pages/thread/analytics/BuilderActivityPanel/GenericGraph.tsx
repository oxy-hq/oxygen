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
import type { AppItemDiff, ItemGroup } from "./types";

const elk = new ELK();

// ── Shared badge used inside row nodes ────────────────────────────────────────

const AppItemBadge = ({ diff }: { diff: AppItemDiff }) => {
  const isAdded = diff.status === "added";
  const isModified = diff.status === "modified";
  const prefix = isAdded ? "+ " : diff.status === "removed" ? "- " : "";
  return (
    <div
      className={cn(
        "rounded border px-2.5 py-1.5",
        isAdded
          ? "border-emerald-500/30 bg-emerald-500/5"
          : isModified
            ? "border-amber-500/30 bg-amber-500/5"
            : "border-destructive/30 bg-destructive/5"
      )}
    >
      <div className='mb-0.5 flex items-center gap-1'>
        <span
          className={cn(
            "h-1.5 w-1.5 rounded-full",
            isAdded ? "bg-emerald-500" : isModified ? "bg-amber-500" : "bg-destructive"
          )}
        />
        <span
          className={cn(
            "font-mono text-[9px] uppercase tracking-wider",
            isAdded ? "text-emerald-500" : isModified ? "text-amber-500" : "text-destructive"
          )}
        >
          {prefix}
          {diff.label}
        </span>
      </div>
      <p className='font-bold text-foreground text-xs'>{diff.title}</p>
      {diff.subtitle && (
        <p className='mt-0.5 font-mono text-[9px] text-muted-foreground'>{diff.subtitle}</p>
      )}
    </div>
  );
};

// ── RF node components ────────────────────────────────────────────────────────

const RfRootNode = ({ data }: NodeProps) => {
  const d = data as {
    rootLabel: string;
    rootTitle: string;
    rootSubtitle?: string;
    addedCount: number;
    modifiedCount: number;
    removedCount: number;
  };
  return (
    <>
      <Handle type='source' position={Position.Bottom} id='bottom' style={HANDLE_STYLE} />
      <div className='rounded-lg border-2 border-primary/50 bg-card px-4 py-3 shadow-sm'>
        <p className='font-mono text-[10px] text-primary/60 uppercase tracking-wider'>
          {d.rootLabel}
        </p>
        <p className='font-bold text-foreground text-sm'>{d.rootTitle}</p>
        {d.rootSubtitle && (
          <p className='mt-0.5 text-[11px] text-muted-foreground'>
            {d.rootSubtitle}
            {d.addedCount > 0 && <span className='ml-1 text-emerald-500'>· +{d.addedCount}</span>}
            {d.modifiedCount > 0 && (
              <span className='ml-1 text-amber-500'>· ~{d.modifiedCount}</span>
            )}
            {d.removedCount > 0 && (
              <span className='ml-1 text-destructive'>· -{d.removedCount}</span>
            )}
          </p>
        )}
      </div>
    </>
  );
};

const RfAppItemNode = ({ data }: NodeProps) => {
  const { diff } = data as { diff: AppItemDiff };
  const isAdded = diff.status === "added";
  const isModified = diff.status === "modified";
  const prefix = isAdded ? "+ " : diff.status === "removed" ? "- " : "";
  return (
    <>
      <Handle type='target' position={Position.Top} id='top' style={HANDLE_STYLE} />
      <div
        className={cn(
          "rounded-lg border-2 bg-card px-4 py-3 shadow-sm",
          isAdded
            ? "border-emerald-500/40"
            : isModified
              ? "border-amber-500/40"
              : "border-destructive/40"
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
            {diff.label}
          </span>
        </div>
        <p className='font-bold text-foreground text-sm'>{diff.title}</p>
        {diff.subtitle && (
          <p className='mt-0.5 font-mono text-[10px] text-muted-foreground'>{diff.subtitle}</p>
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
        {diff.children && diff.children.length > 0 && (
          <div className='mt-2 flex flex-wrap gap-1.5'>
            {diff.children.map((child) => (
              <AppItemBadge key={child.key} diff={child} />
            ))}
          </div>
        )}
      </div>
    </>
  );
};

// nodeTypes must be stable (defined outside components)
const nodeTypes = {
  root: RfRootNode,
  appItem: RfAppItemNode
};

// ── GenericGraph ──────────────────────────────────────────────────────────────

export interface GenericGraphProps {
  change: BuilderFileChange;
  graphLabel: string;
  rootLabel: string;
  rootTitle: string;
  rootSubtitle?: string;
  changedItems: AppItemDiff[];
  itemGroups?: ItemGroup[];
}

export const GenericGraph = ({
  change,
  graphLabel,
  rootLabel,
  rootTitle,
  rootSubtitle,
  changedItems,
  itemGroups
}: GenericGraphProps) => {
  const { theme } = useTheme();
  const allItems = useMemo(
    () => (itemGroups ? itemGroups.flatMap((g) => g.items) : changedItems),
    [itemGroups, changedItems]
  );

  const addedCount = allItems.filter((d) => d.status === "added").length;
  const modifiedCount = allItems.filter((d) => d.status === "modified").length;
  const removedCount = allItems.filter((d) => d.status === "removed").length;

  const baseNodes = useMemo<RFNode[]>(() => {
    const nodes: RFNode[] = [];
    nodes.push({
      id: "root",
      type: "root",
      position: { x: 0, y: 0 },
      width: NODE_W,
      data: { rootLabel, rootTitle, rootSubtitle, addedCount, modifiedCount, removedCount }
    });
    allItems.forEach((diff, i) => {
      nodes.push({
        id: `item-${i}`,
        type: "appItem",
        position: { x: 0, y: 0 },
        width: NODE_W,
        data: { diff }
      });
    });
    return nodes;
  }, [rootLabel, rootTitle, rootSubtitle, addedCount, modifiedCount, removedCount, allItems]);

  const rfEdges = useMemo<RFEdge[]>(
    () =>
      allItems.map((diff, i) =>
        makeEdge(`root-item-${i}`, "root", `item-${i}`, {
          sourceHandle: "bottom",
          targetHandle: "top",
          status: diff.status
        })
      ),
    [allItems]
  );

  const [layoutedNodes, setLayoutedNodes] = useState<RFNode[]>([]);
  const [layoutedEdges, setLayoutedEdges] = useState<RFEdge[]>([]);
  const [rfInstance, setRfInstance] = useState<{ fitView: (opts?: object) => void } | null>(null);
  const [layoutFailed, setLayoutFailed] = useState(false);

  useEffect(() => {
    if (baseNodes.length === 0) return;

    // Layout is async and the inputs change while the builder streams: a slower,
    // earlier layout must not overwrite the result of a newer one.
    let cancelled = false;

    const elkNodes = baseNodes.map((node) => {
      const diff = node.type === "appItem" ? (node.data as { diff: AppItemDiff }).diff : null;
      const changesCount = diff?.changes?.length ?? 0;
      const childrenCount = diff?.children?.length ?? 0;
      const estimatedHeight =
        node.type === "root"
          ? 80
          : 100 +
            changesCount * 18 +
            (childrenCount > 0 ? Math.ceil(childrenCount / 3) * 70 + 16 : 0);
      return { id: node.id, width: NODE_W, height: estimatedHeight };
    });

    elk
      .layout({
        id: "generic-graph",
        layoutOptions: {
          "elk.algorithm": "layered",
          "elk.direction": "DOWN",
          "elk.layered.layering.strategy": "COFFMAN_GRAHAM",
          "elk.layered.layering.coffmanGraham.layerBound": "3",
          "elk.spacing.nodeNode": "20",
          "elk.layered.spacing.nodeNodeBetweenLayers": "60"
        },
        children: elkNodes,
        edges: rfEdges.map((e) => ({ id: e.id, sources: [e.source], targets: [e.target] }))
      })
      .then((layout) => {
        if (cancelled) return;
        setLayoutFailed(false);
        setLayoutedNodes(
          baseNodes.map((node) => {
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
        console.error("Generic graph layout failed", error);
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
            {graphLabel}
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
          {change.status === "pending" && allItems.length > 0 && (
            <span className='rounded bg-emerald-500/15 px-1.5 py-0.5 font-bold text-[10px] text-emerald-600 dark:text-emerald-400'>
              +{allItems.length} pending
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
