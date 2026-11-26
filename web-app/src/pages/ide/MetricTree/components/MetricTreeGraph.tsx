import type { Node as RFNode } from "@xyflow/react";
import { useEffect, useMemo, useState } from "react";
import { Label } from "@/components/ui/shadcn/label";
import { Switch } from "@/components/ui/shadcn/switch";
import type { MetricTree } from "@/types/metricTree";
import { GraphCanvas, type WaypointMap } from "../../components/semanticGraph";
import { layoutWithElk, metricTreeToFlow } from "../graphLayout";
import { type ScenarioNodeData, scenarioEdgeOpacity } from "../scenario/nodeValue";
import { ScenarioNode } from "../scenario/ScenarioNode";
import { MetricMeasureNode } from "./MetricMeasureNode";
import { deriveNodeRoles } from "./nodeRoles";

const nodeTypes = { "metric-measure": MetricMeasureNode, "scenario-measure": ScenarioNode };

/** The tree can run to hundreds of measures — well past what the World Model's
 *  0.3 floor can fit on screen — so this canvas alone lowers it. */
const MIN_ZOOM = 0.05;

interface MetricTreeGraphProps {
  tree: MetricTree;
  selectedId: string | null;
  onSelect: (id: string) => void;
  /** Clicking empty canvas clears the selection, as it does in the World Model. */
  onClearSelection?: () => void;
  scenario?: Map<string, ScenarioNodeData>;
}

export function MetricTreeGraph({
  tree,
  selectedId,
  onSelect,
  onClearSelection,
  scenario
}: MetricTreeGraphProps) {
  const [hideOrphans, setHideOrphans] = useState(true);

  const filteredTree = useMemo<MetricTree>(() => {
    if (!hideOrphans) return tree;
    const connected = new Set<string>();
    for (const e of tree.edges) {
      connected.add(e.from);
      connected.add(e.to);
    }
    return { ...tree, nodes: tree.nodes.filter((n) => connected.has(n.id)) };
  }, [tree, hideOrphans]);

  const roles = useMemo(() => deriveNodeRoles(filteredTree), [filteredTree]);

  // Positions come from a structural pass — TREE SHAPE ONLY — so relayout
  // never fires on a scenario propagation update, or on a selection. ELK reads
  // `id`/`width`/`height` off these nodes and nothing else; `selectedId` and
  // `roles` reach only `data`, and both node variants share the same
  // dimensions, so the positions computed here are valid for every render
  // below, scenario mode included.
  //
  // Passing them in anyway is what made every lever pin re-run ELK: the memo
  // invalidated, the effect nulled `positions`, the canvas showed "Laying
  // out…" and `fitView` threw the viewport away. In scenario mode a click IS
  // the pin, and pinning the first lever also flips the node type — so this
  // fired twice on the feature's opening interaction.
  const structural = useMemo(() => metricTreeToFlow(filteredTree, null), [filteredTree]);

  const [positions, setPositions] = useState<Map<string, RFNode["position"]> | null>(null);
  const [waypointMap, setWaypointMap] = useState<WaypointMap>(new Map());

  useEffect(() => {
    let cancelled = false;
    setPositions(null);
    layoutWithElk(structural.nodes, structural.edges)
      .then(({ nodes: laidOut, waypointMap: wm }) => {
        if (cancelled) return;
        setPositions(new Map(laidOut.map((n) => [n.id, n.position])));
        setWaypointMap(wm);
      })
      .catch((error) => {
        console.error("metric tree layout failed", error);
        if (!cancelled) {
          setPositions(new Map(structural.nodes.map((n) => [n.id, n.position])));
          setWaypointMap(new Map());
        }
      });
    return () => {
      cancelled = true;
    };
  }, [structural]);

  // Data (including scenario values) is recomputed on every render — cheap
  // object construction, no relayout — and merged with the positions above.
  const { nodes: dataNodes, edges: dataEdges } = useMemo(
    () => metricTreeToFlow(filteredTree, selectedId, roles, scenario),
    [filteredTree, selectedId, roles, scenario]
  );

  // Hand each edge the waypoints ELK routed for it, so edges bend around node
  // bodies instead of cutting through them — see `GraphEdge`.
  //
  // In scenario mode an edge also recedes with its endpoints. A fully lit edge
  // running into a dimmed card reads as "the scenario propagated along here",
  // which is the opposite of what a dimmed card means.
  const edges = useMemo(
    () =>
      dataEdges.map((e) => {
        const waypoints = waypointMap.get(e.id);
        if (!scenario) return { ...e, data: { ...e.data, waypoints } };
        const opacity = scenarioEdgeOpacity(
          scenario.get(e.source)?.state,
          scenario.get(e.target)?.state,
          (e.style?.opacity as number | undefined) ?? 1
        );
        return { ...e, data: { ...e.data, waypoints }, style: { ...e.style, opacity } };
      }),
    [dataEdges, waypointMap, scenario]
  );

  const positioned = useMemo<RFNode[] | null>(() => {
    if (!positions) return null;
    return dataNodes.map((node) => ({
      ...node,
      position: positions.get(node.id) ?? node.position
    }));
  }, [dataNodes, positions]);

  if (tree.nodes.length === 0) {
    return (
      <div className='flex h-full items-center justify-center text-muted-foreground text-sm'>
        No measures found in this workspace's semantic model.
      </div>
    );
  }

  const orphanCount = tree.nodes.length - filteredTree.nodes.length;

  const orphanToggle = (
    <div className='absolute top-3 right-3 z-10 flex items-center gap-2 border border-border bg-card/90 px-3 py-1.5 backdrop-blur'>
      <Switch
        id='metric-tree-hide-orphans'
        checked={hideOrphans}
        onCheckedChange={setHideOrphans}
      />
      <Label htmlFor='metric-tree-hide-orphans' className='cursor-pointer text-xs'>
        Hide unconnected
        {hideOrphans && orphanCount > 0 && (
          <span className='ml-1 text-muted-foreground'>({orphanCount})</span>
        )}
      </Label>
    </div>
  );

  if (filteredTree.nodes.length === 0) {
    return (
      <div className='relative h-full w-full'>
        {orphanToggle}
        <div className='flex h-full flex-col items-center justify-center gap-1 text-muted-foreground text-sm'>
          <p>All measures are unconnected.</p>
          <p className='text-xs'>Toggle "Hide unconnected" off to see them.</p>
        </div>
      </div>
    );
  }

  if (positioned === null) {
    return (
      <div className='relative h-full w-full'>
        {orphanToggle}
        <div className='flex h-full items-center justify-center text-muted-foreground text-xs'>
          Laying out…
        </div>
      </div>
    );
  }

  return (
    <GraphCanvas
      flowKey={`${filteredTree.nodes.length}-${edges.length}`}
      nodes={positioned}
      edges={edges}
      nodeTypes={nodeTypes}
      minZoom={MIN_ZOOM}
      onNodeClick={(_event, node) => onSelect(node.id)}
      onPaneClick={onClearSelection}
    >
      {orphanToggle}
    </GraphCanvas>
  );
}
