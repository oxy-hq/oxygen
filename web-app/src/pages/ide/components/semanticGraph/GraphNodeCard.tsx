import { Handle, Position } from "@xyflow/react";
import type { ReactNode } from "react";
import { cn } from "@/libs/shadcn/utils";
import { NODE_WIDTH } from "./constants";

export interface GraphNodeCardProps {
  children: ReactNode;
  /** The graph's current focus. Drawn as an accent ring + glow. */
  selected?: boolean;
  /** Outside the active cluster: keeps full opacity, drops only the accent, so
   *  the highlighted cluster stands out without anything becoming unreadable. */
  dimmed?: boolean;
  /** Softly pushed behind an active foreground (e.g. a measure breakdown). */
  blurred?: boolean;
  width?: number;
  className?: string;
  "data-testid"?: string;
}

/**
 * The card shell every semantic-graph node is drawn in — one border idiom, one
 * accent colour, one set of selected/dimmed/blurred states.
 *
 * Nodes differ in what they put *inside* the card (an entity's measure chips, a
 * measure's role and value), never in how the card itself is drawn. Anything
 * role- or type-specific belongs in `children`, expressed as a symbol or a
 * micro-label — not as another border or background colour, which is how these
 * two surfaces drifted apart in the first place.
 */
export function GraphNodeCard({
  children,
  selected = false,
  dimmed = false,
  blurred = false,
  width = NODE_WIDTH,
  className,
  "data-testid": testId
}: GraphNodeCardProps) {
  return (
    <div
      className={cn(
        "flex cursor-pointer select-none flex-col gap-1 border bg-card p-2",
        "transition-all duration-250 ease-out",
        dimmed ? "border-border-subtle" : "border-info/60 hover:border-info",
        selected && "ring-2 ring-info/60",
        blurred && "opacity-40 blur-[1.5px]",
        className
      )}
      style={{ width }}
      data-testid={testId}
    >
      {children}
    </div>
  );
}

/**
 * The four invisible handles a card needs so ELK-routed edges can attach top
 * and bottom in either direction. Ids are stable (`top-in` / `bottom-out` …) so
 * an edge can pick a side explicitly when it needs to.
 */
export function GraphNodeHandles() {
  return (
    <>
      <Handle id='top-in' type='target' position={Position.Top} className='opacity-0' />
      <Handle id='top-out' type='source' position={Position.Top} className='opacity-0' />
      <Handle id='bottom-in' type='target' position={Position.Bottom} className='opacity-0' />
      <Handle id='bottom-out' type='source' position={Position.Bottom} className='opacity-0' />
    </>
  );
}
