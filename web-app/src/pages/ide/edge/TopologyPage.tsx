import {
  Background,
  ControlButton,
  Controls,
  type Edge as FlowEdge,
  type Node as FlowNode,
  Handle,
  type NodeProps,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow
} from "@xyflow/react";
import { ArrowDown, ArrowRight, Building2, Camera, Cpu, MapPin } from "lucide-react";
import type React from "react";
import { useEffect, useMemo } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { Badge } from "@/components/ui/shadcn/badge";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import useCameras from "@/hooks/api/cameras/useCameras";
import useEdgeBoxes from "@/hooks/api/cameras/useEdgeBoxes";
import useSites from "@/hooks/api/cameras/useSites";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * Topology — UniFi-style network map of the fleet. Four tiers:
 * tenant → sites → edge boxes → cameras. Layout direction is
 * operator-toggleable (LR default, TB alternative) via the header
 * control; xyflow's built-in `<Controls />` provides zoom/fit/lock.
 *
 * Layout math is plain JS (no extra layout dep) because the tree
 * shape is regular and depth-bounded. Bottom-up Y/X centering puts
 * related nodes adjacent without zig-zagging across columns.
 *
 * The Topology tab deliberately ignores the global site picker —
 * the value of this view IS the cross-site structure. Operators
 * who want a single site go to Devices / Playback.
 */

type Tier = "tenant" | "site" | "box" | "camera";
type Direction = "LR" | "TB";

type EdgeNodeData = {
  tier: Tier;
  label: string;
  sublabel?: string;
  status?: "ok" | "warn" | "down" | "neutral";
  href?: string;
};

const CARD_WIDTH = 200;
const CARD_HEIGHT = 64;
const TIER_GAP = 80;
const SIBLING_GAP = 24;
const TIER_INDEX: Record<Tier, number> = {
  tenant: 0,
  site: 1,
  box: 2,
  camera: 3
};

const tierIcon = (tier: Tier) => {
  switch (tier) {
    case "tenant":
      return Building2;
    case "site":
      return MapPin;
    case "box":
      return Cpu;
    case "camera":
      return Camera;
  }
};

const statusRing = (status: EdgeNodeData["status"]) => {
  switch (status) {
    case "ok":
      return "ring-2 ring-emerald-500/40 bg-emerald-500/5";
    case "warn":
      return "ring-2 ring-amber-500/40 bg-amber-500/5";
    case "down":
      return "ring-2 ring-destructive/50 bg-destructive/5";
    default:
      return "ring-1 ring-border bg-card";
  }
};

const makeTopologyNode =
  (direction: Direction): React.FC<NodeProps<FlowNode<EdgeNodeData>>> =>
  ({ data }) => {
    const Icon = tierIcon(data.tier);
    // Handle positions follow the layout direction so smoothstep
    // edges hit the natural near-side of each card.
    const targetPos = direction === "LR" ? Position.Left : Position.Top;
    const sourcePos = direction === "LR" ? Position.Right : Position.Bottom;
    return (
      <div
        className={cn(
          "flex h-16 w-[200px] items-center gap-2 rounded-md px-3 shadow-sm transition-shadow",
          statusRing(data.status),
          data.href && "cursor-pointer hover:shadow-md hover:ring-primary/40"
        )}
        data-testid={`topology-node-${data.tier}`}
      >
        <Handle type='target' position={targetPos} className='!h-2 !w-2 !bg-border' />
        <Icon className='h-4 w-4 shrink-0 text-muted-foreground' />
        <div className='min-w-0 flex-1'>
          <p className='truncate font-medium text-sm leading-tight'>{data.label}</p>
          {data.sublabel && (
            <p className='truncate text-muted-foreground text-xs leading-tight'>{data.sublabel}</p>
          )}
        </div>
        <Handle type='source' position={sourcePos} className='!h-2 !w-2 !bg-border' />
      </div>
    );
  };

/**
 * Map a tier-index + cross-axis-offset to {x, y} based on the
 * current layout direction.
 *
 * LR layout:  x = TIER_INDEX * tier-stride;  y = sibling-offset
 * TB layout:  x = sibling-offset;            y = TIER_INDEX * tier-stride
 */
const positionFor = (tier: Tier, crossAxis: number, direction: Direction) => {
  const tierStride = direction === "LR" ? CARD_WIDTH + TIER_GAP : CARD_HEIGHT + TIER_GAP * 1.5;
  const tierAxis = TIER_INDEX[tier] * tierStride;
  return direction === "LR" ? { x: tierAxis, y: crossAxis } : { x: crossAxis, y: tierAxis };
};

const TopologyContent: React.FC<{
  direction: Direction;
  onToggleDirection: () => void;
}> = ({ direction, onToggleDirection }) => {
  const navigate = useNavigate();
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { workspace } = useCurrentWorkspace();
  const wsId = workspace?.id;

  const { data: sites = [], isLoading: sitesLoading } = useSites(wsId);
  const { data: edgeBoxes = [], isLoading: boxesLoading } = useEdgeBoxes(wsId);
  const { data: cameras = [], isLoading: camerasLoading } = useCameras(wsId);

  const nodeTypes = useMemo(() => ({ topology: makeTopologyNode(direction) }), [direction]);

  const { nodes, edges } = useMemo<{
    nodes: FlowNode<EdgeNodeData>[];
    edges: FlowEdge[];
  }>(() => {
    const ns: FlowNode<EdgeNodeData>[] = [];
    const es: FlowEdge[] = [];

    const playback = ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.EDGE.PLAYBACK;

    // Bottom-up cross-axis layout — assign cross-axis per leaf
    // (camera) first, then center each box on its cameras, sites on
    // boxes, tenant on sites. Keeps related nodes visually adjacent
    // without a layout library; same algorithm regardless of LR/TB.
    const camsByBox = new Map<string, typeof cameras>();
    for (const c of cameras) {
      if (!c.edge_box_id) continue;
      const list = camsByBox.get(c.edge_box_id) ?? [];
      list.push(c);
      camsByBox.set(c.edge_box_id, list);
    }
    const boxesBySite = new Map<string, typeof edgeBoxes>();
    for (const b of edgeBoxes) {
      const list = boxesBySite.get(b.site_id) ?? [];
      list.push(b);
      boxesBySite.set(b.site_id, list);
    }

    const camCross = new Map<string, number>();
    const boxCross = new Map<string, number>();
    const siteCross = new Map<string, number>();
    let cursor = 0;
    const stride = direction === "LR" ? CARD_HEIGHT + SIBLING_GAP : CARD_WIDTH + SIBLING_GAP;

    for (const s of sites) {
      const siteStart = cursor;
      const boxes = boxesBySite.get(s.id) ?? [];
      if (boxes.length === 0) {
        siteCross.set(s.id, cursor);
        cursor += stride;
        continue;
      }
      for (const b of boxes) {
        const cams = camsByBox.get(b.id) ?? [];
        if (cams.length === 0) {
          boxCross.set(b.id, cursor);
          cursor += stride;
          continue;
        }
        const camStart = cursor;
        for (const c of cams) {
          camCross.set(c.id, cursor);
          cursor += stride;
        }
        boxCross.set(b.id, (camStart + (cursor - stride)) / 2);
      }
      const boxRange = boxes
        .map((b) => boxCross.get(b.id))
        .filter((v): v is number => v !== undefined);
      siteCross.set(
        s.id,
        boxRange.length > 0 ? (Math.min(...boxRange) + Math.max(...boxRange)) / 2 : siteStart
      );
    }
    const siteRange = Array.from(siteCross.values());
    const tenantCross =
      siteRange.length > 0 ? (Math.min(...siteRange) + Math.max(...siteRange)) / 2 : 0;

    ns.push({
      id: "tenant",
      type: "topology",
      position: positionFor("tenant", tenantCross, direction),
      data: { tier: "tenant", label: workspace?.name ?? "Workspace", sublabel: "Tenant" }
    });

    for (const s of sites) {
      const cross = siteCross.get(s.id) ?? 0;
      ns.push({
        id: `site-${s.id}`,
        type: "topology",
        position: positionFor("site", cross, direction),
        data: {
          tier: "site",
          label: s.name,
          sublabel: s.region ?? s.timezone,
          status: "neutral",
          href: `${playback}?site=${encodeURIComponent(s.id)}`
        }
      });
      es.push({
        id: `tenant-site-${s.id}`,
        source: "tenant",
        target: `site-${s.id}`,
        type: "smoothstep",
        style: { stroke: "var(--border)" }
      });
    }

    for (const b of edgeBoxes) {
      const cross = boxCross.get(b.id);
      if (cross === undefined) continue;
      const status: EdgeNodeData["status"] =
        b.status === "active" ? "ok" : b.status === "offline" ? "down" : "warn";
      ns.push({
        id: `box-${b.id}`,
        type: "topology",
        position: positionFor("box", cross, direction),
        data: {
          tier: "box",
          label: b.hardware_model || "Edge box",
          sublabel: b.status,
          status,
          href: `${playback}?box=${encodeURIComponent(b.id)}`
        }
      });
      es.push({
        id: `site-${b.site_id}-box-${b.id}`,
        source: `site-${b.site_id}`,
        target: `box-${b.id}`,
        type: "smoothstep",
        style: { stroke: "var(--border)" }
      });
    }

    for (const c of cameras) {
      const cross = camCross.get(c.id);
      if (cross === undefined || !c.edge_box_id) continue;
      const status: EdgeNodeData["status"] =
        c.online === true ? "ok" : c.online === false ? "down" : "neutral";
      ns.push({
        id: `cam-${c.id}`,
        type: "topology",
        position: positionFor("camera", cross, direction),
        data: {
          tier: "camera",
          label: c.name,
          sublabel: c.role ?? "unassigned",
          status,
          href: `${playback}?camera=${encodeURIComponent(c.id)}`
        }
      });
      es.push({
        id: `box-${c.edge_box_id}-cam-${c.id}`,
        source: `box-${c.edge_box_id}`,
        target: `cam-${c.id}`,
        type: "smoothstep",
        style: { stroke: "var(--border)" }
      });
    }

    return { nodes: ns, edges: es };
  }, [sites, edgeBoxes, cameras, workspace?.name, orgSlug, project.id, direction]);

  const handleNodeClick = (_: unknown, node: FlowNode<EdgeNodeData>) => {
    if (node.data.href) navigate(node.data.href);
  };

  // Refit when the layout direction flips — `fitView` on <ReactFlow>
  // only fires once at mount, so without this the camera stays
  // pointed at the old layout and the new positions slide off-screen.
  // Defer one tick so the new node positions are committed first.
  // `useReactFlow()` returns a stable instance, so it's safe (and
  // necessary, per the lint config) to omit from the deps array.
  const reactFlow = useReactFlow();
  // biome-ignore lint/correctness/useExhaustiveDependencies: reactFlow handle is stable
  useEffect(() => {
    const id = window.setTimeout(() => {
      // React Flow's `fitView` only ever resolves (to whether it fitted).
      void reactFlow.fitView({ padding: 0.2, duration: 300 });
    }, 0);
    return () => window.clearTimeout(id);
  }, [direction]);

  const loading = sitesLoading || boxesLoading || camerasLoading;
  const empty = !loading && sites.length === 0;

  if (loading) {
    return (
      <div className='flex h-full w-full items-center justify-center p-6'>
        <Skeleton className='h-64 w-full max-w-3xl' />
      </div>
    );
  }

  if (empty) {
    return (
      <div className='flex h-full w-full flex-col items-center justify-center gap-2 p-6 text-center'>
        <Building2 className='h-8 w-8 text-muted-foreground' />
        <p className='font-medium text-sm'>No sites yet</p>
        <p className='text-muted-foreground text-xs'>
          Add a site from Devices to populate the topology.
        </p>
      </div>
    );
  }

  return (
    // `key={direction}` forces a fresh ReactFlow mount on layout
    // flip. Without it, RF's internal node-position state survives
    // the `nodes` prop change and the toggle visually no-ops even
    // though the new positions are computed correctly.
    <ReactFlow
      key={direction}
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      onNodeClick={handleNodeClick}
      fitView
      fitViewOptions={{ padding: 0.2 }}
      minZoom={0.2}
      maxZoom={1.5}
      proOptions={{ hideAttribution: true }}
    >
      <Background gap={20} size={1} className='bg-muted/30' />
      {/* Theme-aware overrides — xyflow defaults to light grays that
          look wrong in dark mode. The `[&_button]:` arbitrary
          selectors apply to every default control button (zoom in /
          out / fit / lock) plus our custom direction toggle below,
          so they all read consistently against bg-card + foreground. */}
      <Controls
        showInteractive={false}
        className={cn(
          "!overflow-hidden !rounded-md !border !border-border !bg-card !shadow-sm",
          "[&_button]:!border-border [&_button]:!bg-card [&_button]:!fill-foreground",
          "[&_button:hover]:!bg-muted"
        )}
      >
        <ControlButton
          onClick={onToggleDirection}
          title={
            direction === "LR" ? "Switch to top-down layout" : "Switch to left-to-right layout"
          }
          data-testid='topology-dir-toggle'
        >
          {direction === "LR" ? (
            <ArrowDown className='h-3 w-3' />
          ) : (
            <ArrowRight className='h-3 w-3' />
          )}
        </ControlButton>
      </Controls>
    </ReactFlow>
  );
};

const TopologyPage: React.FC = () => {
  // Layout direction persists in URL (`?dir=tb`) so links share the
  // operator's preferred orientation. LR is the default — matches
  // the way customer fleets are usually drawn (tenant on the left).
  const [searchParams, setSearchParams] = useSearchParams();
  const direction: Direction = searchParams.get("dir") === "tb" ? "TB" : "LR";

  const setDirection = (next: Direction) => {
    const sp = new URLSearchParams(searchParams);
    if (next === "LR") sp.delete("dir");
    else sp.set("dir", next);
    setSearchParams(sp, { replace: true });
  };

  const toggleDirection = () => setDirection(direction === "LR" ? "TB" : "LR");

  return (
    <div className='relative h-full min-h-0' data-testid='edge-topology'>
      {/* Status legend — floats over the graph top-left rather than
          consuming a whole header row. The Controls panel
          (zoom/fit/lock + direction toggle) lives bottom-left
          courtesy of xyflow's defaults. */}
      <div className='pointer-events-none absolute top-3 left-3 z-10 flex items-center gap-1.5'>
        <Badge variant='outline' className='gap-1.5 bg-card text-[10px]'>
          <span className='h-1.5 w-1.5 rounded-full bg-emerald-500' /> active
        </Badge>
        <Badge variant='outline' className='gap-1.5 bg-card text-[10px]'>
          <span className='h-1.5 w-1.5 rounded-full bg-amber-500' /> pending
        </Badge>
        <Badge variant='outline' className='gap-1.5 bg-card text-[10px]'>
          <span className='h-1.5 w-1.5 rounded-full bg-destructive' /> offline
        </Badge>
      </div>
      <ReactFlowProvider>
        <TopologyContent direction={direction} onToggleDirection={toggleDirection} />
      </ReactFlowProvider>
    </div>
  );
};

export default TopologyPage;
