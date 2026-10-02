import type { Edge as RFEdge, Node as RFNode } from "@xyflow/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useWmMeasureBreakdown } from "@/hooks/api/useWorldModel";
import type { WmComputedMeasure, WmSelection, WorldModel } from "@/types/worldModel";
import { GRAPH_EDGE_TYPE, GraphCanvas } from "../../components/semanticGraph";
import {
  breakdownNodeToComputedMeasure,
  buildBreakdownEdges,
  buildLayoutSizeMap,
  buildViewToEntityIds,
  composedSelfHandles,
  composedTargetHandles,
  contributorSourceHandles,
  EXPANDED_NODE_WIDTH,
  groupBreakdownContributorsByEntity,
  layoutWithElk,
  NODE_HANDLES,
  NODE_HEIGHT_COLLAPSED,
  NODE_WIDTH,
  type WaypointMap,
  worldModelToFlow
} from "../worldModelLayout";
import { WorldModelEntityNode } from "./WorldModelEntityNode";
import { WorldModelExpandedEntityNode } from "./WorldModelExpandedEntityNode";

const nodeTypes = {
  "wm-entity": WorldModelEntityNode,
  "wm-entity-expanded": WorldModelExpandedEntityNode
};

/** Debounce (ms) for size-driven reflows so streamed filter counts and the
 *  async breakdown load coalesce into a single animated layout move. */
const RELAYOUT_DEBOUNCE_MS = 160;

interface WorldModelGraphProps {
  model: WorldModel;
  selection: WmSelection;
  filterCounts?: Record<
    string,
    { matched: number; total: number; sample?: string[]; sample_keys?: string[] }
  > | null;
  isCountLoading?: boolean;
  filterSeedEntityId?: string | null;
  seedComputedMeasures?: WmComputedMeasure[] | null;
  expandedEntityId?: string | null;
  breakdownMeasure?: string | null;
  instanceKey?: string | null;
  onExpandEntity?: (id: string | null, measure?: string | null) => void;
  onSelectEntity: (id: string) => void;
  onSelectPromotion: (from: string, to: string) => void;
  onOpenPicker: (entityId: string, pos: { x: number; y: number }) => void;
  onClearSelection: () => void;
  onSelectChildInstance: (entityId: string, key: string, display: string) => void;
  onBrowseSamples: (entityId: string, position: { x: number; y: number }) => void;
}

export function WorldModelGraph({
  model,
  selection,
  filterCounts,
  isCountLoading = false,
  filterSeedEntityId = null,
  seedComputedMeasures = null,
  expandedEntityId = null,
  breakdownMeasure = null,
  instanceKey = null,
  onExpandEntity,
  onSelectEntity,
  onSelectPromotion,
  onOpenPicker,
  onClearSelection,
  onSelectChildInstance,
  onBrowseSamples
}: WorldModelGraphProps) {
  // Entity visibility is driven entirely by `.world-model.yml` (applied
  // server-side); the graph renders every entity the backend returns, including
  // ones with no relationships.

  // Topology-only layout trigger — selection changes don't re-run ELK.
  const { nodes: rawNodes, edges: layoutEdges } = useMemo(
    () => worldModelToFlow(model, null),
    [model]
  );

  // Selection-aware overlay — instant, no ELK call.
  const { nodes: selectionNodes, edges } = useMemo(
    () => worldModelToFlow(model, selection),
    [model, selection]
  );

  const selectionDataMap = useMemo(
    () => new Map(selectionNodes.map((n) => [n.id, n.data])),
    [selectionNodes]
  );

  const [positioned, setPositioned] = useState<RFNode[] | null>(null);
  const [waypointMap, setWaypointMap] = useState<WaypointMap>(new Map());

  // When a measure breakdown is active, its non-root nodes are shown on
  // whichever entity card actually owns each contributor measure — grouped
  // by entity (resolved from the breakdown's view names) rather than spawned
  // as synthetic floating nodes.
  const { data: breakdown, error: breakdownError } = useWmMeasureBreakdown(
    expandedEntityId,
    instanceKey,
    breakdownMeasure
  );
  const viewToEntityIds = useMemo(() => buildViewToEntityIds(model.entities), [model]);
  const contributorsByEntity = useMemo(
    () => groupBreakdownContributorsByEntity(breakdown ?? null, viewToEntityIds),
    [breakdown, viewToEntityIds]
  );
  const breakdownEdges = useMemo(
    () =>
      expandedEntityId
        ? buildBreakdownEdges(expandedEntityId, breakdown ?? null, viewToEntityIds)
        : [],
    [expandedEntityId, breakdown, viewToEntityIds]
  );

  // Size-aware layout: each card is laid out at its real (possibly grown) box so
  // neighbors reflow to make room for expanded / measure / sample cards instead
  // of being overlapped. The expanded card's row count is the root measure plus
  // any same-card contributors (null while the breakdown loads → placeholder).
  const expandedRowCount = useMemo(() => {
    if (!expandedEntityId || !breakdown) return null;
    const hasRoot = breakdown.nodes.some((bn) => bn.id === breakdown.root);
    if (!hasRoot) return null;
    return 1 + (contributorsByEntity.get(expandedEntityId)?.length ?? 0);
  }, [expandedEntityId, breakdown, contributorsByEntity]);

  const sizeMap = useMemo(
    () =>
      buildLayoutSizeMap(
        model.entities.map((e) => e.id),
        {
          expandedEntityId,
          expandedRowCount,
          filterSeedEntityId,
          seedComputedMeasures,
          contributorsByEntity,
          filterCounts: filterCounts ?? null,
          isCountLoading
        }
      ),
    [
      model,
      expandedEntityId,
      expandedRowCount,
      filterSeedEntityId,
      seedComputedMeasures,
      contributorsByEntity,
      filterCounts,
      isCountLoading
    ]
  );

  // A primitive key so the layout effect only reflows when a card's reserved box
  // actually changes bucket — not on every streamed count update that leaves
  // every size unchanged.
  const sizeKey = useMemo(
    () =>
      [...sizeMap.entries()]
        .map(([id, s]) => `${id}:${s.width}:${s.height}`)
        .sort()
        .join("|"),
    [sizeMap]
  );

  // Read the latest sizeMap inside the (sizeKey-gated) effect without widening
  // its deps to the map's identity.
  const sizeMapRef = useRef(sizeMap);
  sizeMapRef.current = sizeMap;

  // Run ELK. Two triggers, different feel:
  //  - Topology change (new node set): clear positions, show the placeholder,
  //    lay out immediately.
  //  - Size-only change (selection / expansion / settled counts): keep the
  //    current positions on screen and reflow after a short debounce, so nodes
  //    animate to their new slots (see the .react-flow__node transition) and
  //    streamed counts coalesce into one move instead of thrashing.
  const layoutSeqRef = useRef(0);
  const prevNodeIdsRef = useRef<string>("");
  // sizeKey is an intentional reflow gate: the effect reads the live sizes via
  // sizeMapRef, so it must re-run when the bucketed size key changes without
  // depending on the map's identity (which would thrash on every count update).
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional sizeKey reflow gate, read via sizeMapRef
  useEffect(() => {
    const nodeIds = rawNodes.map((n) => n.id).join(",");
    const topologyChanged = nodeIds !== prevNodeIdsRef.current;
    prevNodeIdsRef.current = nodeIds;
    if (topologyChanged) setPositioned(null);

    const seq = ++layoutSeqRef.current;
    const run = () => {
      layoutWithElk(rawNodes, layoutEdges, (id) => sizeMapRef.current.get(id))
        .then(({ nodes: laidOut, waypointMap: wm }) => {
          if (seq !== layoutSeqRef.current) return;
          setPositioned(laidOut);
          setWaypointMap(wm);
        })
        .catch((err) => {
          console.error("world model layout failed", err);
          if (seq !== layoutSeqRef.current) return;
          setPositioned(rawNodes);
          setWaypointMap(new Map());
        });
    };
    const timer = setTimeout(run, topologyChanged ? 0 : RELAYOUT_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [rawNodes, layoutEdges, sizeKey]);

  // Derive the currently highlighted entity id from selection
  const selectedEntityId =
    selection?.kind === "entity"
      ? selection.entityId
      : selection?.kind === "dimension" || selection?.kind === "measure"
        ? selection.entityId
        : selection?.kind === "instance"
          ? selection.entityId
          : null;

  // Merge: positions from layout + selection state + filter counts + chip data.
  const isInstanceSelection = selection?.kind === "instance";
  const displayNodes = useMemo(
    () =>
      positioned?.map((n) => {
        const selData = selectionDataMap.get(n.id) ?? n.data;
        const isSeed = n.id === filterSeedEntityId;
        const isExpanded = n.id === expandedEntityId;

        // Expanded node: shrink to just the measure being broken down, plus any
        // of its direct components that happen to live on this same entity.
        if (isExpanded) {
          const rootNode = breakdown?.nodes.find((bn) => bn.id === breakdown.root) ?? null;
          const breakdownMeasures = rootNode
            ? [breakdownNodeToComputedMeasure(rootNode), ...(contributorsByEntity.get(n.id) ?? [])]
            : null;
          return {
            ...n,
            type: "wm-entity-expanded",
            width: EXPANDED_NODE_WIDTH,
            height: undefined,
            zIndex: 10,
            // Seed the composite rows' handles (left targets for cross-entity
            // contributors, right source+target for same-card composition edges)
            // so both edge kinds render before the card is measured (see
            // NODE_HANDLES).
            handles: [
              ...NODE_HANDLES,
              ...composedTargetHandles(breakdownMeasures ?? []),
              ...composedSelfHandles(breakdownMeasures ?? [])
            ],
            data: {
              ...selData,
              breakdownMeasure,
              breakdownMeasures,
              breakdownFailed: !!breakdownError,
              instanceKey,
              onExpandEntity,
              dimmed: false
            }
          };
        }

        // A card showing an active breakdown contributor takes priority over the
        // filter-seed's own measure chips — the user is mid-drilldown.
        const contributorMeasures = contributorsByEntity.get(n.id) ?? null;

        // Other nodes drop back when any expansion is active, focusing the card —
        // except a card hosting a contributor, which stays lit to match its edge.
        const dimmed = expandedEntityId
          ? contributorMeasures === null
          : isInstanceSelection
            ? n.id !== selectedEntityId && filterCounts?.[n.id] === undefined
            : (selData.dimmed as boolean);

        // Cards showing descendant sample chips grow to fit; the rest stay compact
        // so layout spacing is unaffected.
        const hasSamples = (filterCounts?.[n.id]?.sample?.length ?? 0) > 0;

        return {
          ...n,
          type: "wm-entity",
          width: NODE_WIDTH,
          height: hasSamples ? undefined : NODE_HEIGHT_COLLAPSED,
          // A contributor card exposes a per-measure source handle on each row;
          // seed their bounds so the breakdown edge renders on the first frame
          // instead of racing the ResizeObserver (see NODE_HANDLES).
          handles: contributorMeasures
            ? [...NODE_HANDLES, ...contributorSourceHandles(contributorMeasures)]
            : NODE_HANDLES,
          data: {
            ...selData,
            filterCount: filterCounts?.[n.id] ?? null,
            isCountLoading,
            dimmed,
            // While a breakdown is on screen, push every card that isn't part of
            // it (the expanded card + its contributors) back with a soft blur so
            // the decomposition reads as the foreground.
            blurred: !!expandedEntityId && contributorMeasures === null,
            seedComputedMeasures: contributorMeasures ?? (isSeed ? seedComputedMeasures : null),
            isContributorCard: contributorMeasures !== null,
            onExpandEntity,
            onSelectChildInstance,
            onBrowseSamples
          }
        };
      }) ?? null,
    [
      positioned,
      selectionDataMap,
      filterCounts,
      isCountLoading,
      isInstanceSelection,
      selectedEntityId,
      filterSeedEntityId,
      seedComputedMeasures,
      expandedEntityId,
      breakdownMeasure,
      breakdown,
      breakdownError,
      contributorsByEntity,
      instanceKey,
      onExpandEntity,
      onSelectChildInstance,
      onBrowseSamples
    ]
  );

  // Overlay edge opacity to match progressive node reveal during instance selection.
  // Embed Graphviz-computed SVG paths so edges route around node bodies.
  const displayEdges = useMemo(() => {
    return edges.map((e) => {
      // A breakdown owns the foreground: fade the structural topology edges so
      // only the dashed contributor edges stand out. Otherwise fall back to the
      // instance-reveal opacity, then the edge's own.
      const opacity = expandedEntityId
        ? 0.12
        : isInstanceSelection
          ? filterCounts?.[e.source] !== undefined && filterCounts?.[e.target] !== undefined
            ? 0.6
            : 0.25
          : (e.style?.opacity as number | undefined);
      return {
        ...e,
        type: GRAPH_EDGE_TYPE,
        data: { ...e.data, waypoints: waypointMap.get(e.id) },
        style: { ...e.style, ...(opacity !== undefined ? { opacity } : {}) }
      };
    });
  }, [edges, filterCounts, isInstanceSelection, expandedEntityId, waypointMap]);

  // Breakdown edges connect existing, already-positioned entity nodes — no
  // extra layout pass needed, just append them to what's rendered.
  const finalEdges = useMemo(
    () => [...displayEdges, ...breakdownEdges],
    [displayEdges, breakdownEdges]
  );

  const handleNodeClick = (event: React.MouseEvent, node: RFNode) => {
    // The expanded card owns its own interactions (close button, tree); ignore body clicks.
    if (node.id === expandedEntityId) return;
    // First click both selects the entity and opens its instance picker — no
    // second click required.
    onSelectEntity(node.id);
    onOpenPicker(node.id, { x: event.clientX, y: event.clientY });
  };

  const handleEdgeClick = (_event: React.MouseEvent, edge: RFEdge) => {
    if (edge.data?.isBreakdownEdge) return;
    onSelectPromotion(edge.source, edge.target);
  };

  if (model.entities.length === 0) {
    return (
      <div className='flex h-full flex-col items-center justify-center gap-1 text-muted-foreground text-sm'>
        <p>No entities to show.</p>
        <p className='text-xs'>
          Define entities in your .view.yml files (and list them in .world-model.yml) to see them
          here.
        </p>
      </div>
    );
  }

  if (displayNodes === null) {
    return (
      <div className='flex h-full items-center justify-center text-muted-foreground text-xs'>
        Laying out…
      </div>
    );
  }

  return (
    <GraphCanvas
      flowKey={`${model.entities.length}-${edges.length}`}
      nodes={displayNodes}
      edges={finalEdges}
      nodeTypes={nodeTypes}
      onNodeClick={handleNodeClick}
      onEdgeClick={handleEdgeClick}
      onPaneClick={onClearSelection}
    />
  );
}
