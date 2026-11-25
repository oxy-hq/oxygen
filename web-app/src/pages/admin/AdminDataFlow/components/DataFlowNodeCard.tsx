import { Handle, type NodeProps, Position } from "@xyflow/react";
import {
  AppWindow,
  ChevronsUpDown,
  Database,
  Layers,
  type LucideIcon,
  Plug,
  SquareFunction,
  Table2,
  Waypoints
} from "lucide-react";
import { STATUS_CLASSES } from "@/components/airway/LineageGraph";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { DataFlowNode, DataFlowNodeType } from "@/types/dataFlow";
import type { NodeFlag } from "../dataFlowModel";

export const NODE_WIDTH = 208;
export const NODE_HEIGHT = 52;

const ICON: Record<DataFlowNodeType, LucideIcon> = {
  source: Plug,
  pipeline: Waypoints,
  store: Database,
  schema: Layers,
  table: Table2,
  app: AppWindow,
  function: SquareFunction
};

export type DataFlowNodeData = {
  node: DataFlowNode;
  caption: string;
  flags: NodeFlag[];
  /** The server's focus: the node the neighbourhood is drawn around. */
  focused: boolean;
  /** Clicked: its edges are lit, the rest dimmed. */
  selected: boolean;
  /** Something else is selected and this is not next to it. */
  dimmed: boolean;
  /** The search matched it. */
  matched?: boolean;
  /** A folded group: how many nodes it holds. Clicking it opens it. */
  count?: number;
};

/**
 * Four invisible handles: data enters on the left and leaves on the right when it
 * runs left to right, and the other way round for a link that runs back (an app
 * writing a store to its left).
 */
const HANDLES = (
  <>
    <Handle id='in-l' type='target' position={Position.Left} className='!opacity-0' />
    <Handle id='out-l' type='source' position={Position.Left} className='!opacity-0' />
    <Handle id='in-r' type='target' position={Position.Right} className='!opacity-0' />
    <Handle id='out-r' type='source' position={Position.Right} className='!opacity-0' />
  </>
);

function frameClasses(d: DataFlowNodeData): string {
  if (d.flags.some((f) => f.tone === "danger")) return STATUS_CLASSES.error;
  if (d.focused) return STATUS_CLASSES.done;
  if (d.flags.length > 0) return cn("border-dashed", ADMIN_TONE.warn.text);
  return d.node.type === "app" ? "border-primary" : "border-border";
}

/** One node of the canvas: its kind's icon, its name, and where it lives. */
export const DataFlowNodeCard = ({ data }: NodeProps) => {
  const d = data as DataFlowNodeData;
  const Icon = ICON[d.node.type];
  const flag = d.flags[0];
  return (
    <div
      className={cn(
        // Cards arrive with a short fade and rise, so opening a group reads as the group unfolding.
        "flex h-13 w-52 cursor-pointer items-center gap-2 rounded-md border bg-background px-2.5 shadow-sm transition-[opacity,translate] duration-(--duration-slow) starting:translate-y-1 starting:opacity-0 motion-reduce:starting:translate-y-0",
        frameClasses(d),
        d.count !== undefined && "border-dashed bg-muted/40",
        d.matched && "ring-2 ring-primary",
        d.selected && "ring-2 ring-primary/40",
        d.dimmed && "opacity-35"
      )}
      title={
        d.count !== undefined ? "Click to expand" : flag ? `${d.node.id}\n${flag.hint}` : d.node.id
      }
      data-testid={
        d.count !== undefined ? "admin-data-flow-group" : `admin-data-flow-node-${d.node.type}`
      }
    >
      {HANDLES}
      <Icon className='size-3.5 shrink-0 text-muted-foreground' />
      <div className='min-w-0 flex-1'>
        <div className='truncate font-medium text-foreground text-xs'>{d.node.label}</div>
        <div className='truncate text-[10px] text-muted-foreground'>
          {flag ? <span className={ADMIN_TONE[flag.tone].text}>{flag.label}</span> : d.caption}
        </div>
      </div>
      {d.count !== undefined && (
        <ChevronsUpDown className='size-3 shrink-0 text-muted-foreground' />
      )}
    </div>
  );
};
