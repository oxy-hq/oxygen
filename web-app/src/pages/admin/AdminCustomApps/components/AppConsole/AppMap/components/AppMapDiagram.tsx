import { CircleCheck, Clock, Globe, type LucideIcon, Play, Webhook, Workflow } from "lucide-react";
import { useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppMapLayout, Box } from "../appMapLayout";
import {
  type AppMap,
  type EntryKind,
  functionId,
  type MapEdge,
  type MapEntry,
  type MapFunction,
  type MapTarget
} from "../appMapModel";
import { entryText, runsLabel, targetSummary } from "../describe";

const ENTRY_ICON: Record<EntryKind, LucideIcon> = {
  route: Globe,
  webhook: Webhook,
  schedule: Clock,
  airway: Workflow,
  check: CircleCheck,
  manual: Play
};

const HEADING = "font-medium text-[10px] text-muted-foreground uppercase tracking-[0.16em]";
const DANGER = ADMIN_TONE.danger;

const place = (box: Box | undefined) =>
  box ? { left: box.x, top: box.y, width: box.width, height: box.height } : { display: "none" };

/**
 * The app map drawn: HTML nodes over one SVG of edges, both positioned from
 * {@link AppMapLayout}. Hovering or focusing a function dims every edge it is not on;
 * the open function (`?fn=`) is held that way when nothing is hovered.
 *
 * The SVG and the entry and target nodes are hidden from assistive technology, which
 * reads the same map from the list beneath; the function nodes stay reachable,
 * because they are controls.
 */
export const AppMapDiagram = ({
  map,
  layout,
  selected,
  onOpenFunction
}: {
  map: AppMap;
  layout: AppMapLayout;
  selected: string | null;
  onOpenFunction: (name: string) => void;
}) => {
  const [hovered, setHovered] = useState<string | null>(null);
  const active = hovered ?? (selected ? functionId(selected) : null);
  const lit = (edge: MapEdge) => !active || edge.from === active || edge.to === active;
  const litTargets = new Set(map.edges.filter(lit).map((e) => e.to));

  return (
    <div
      className='relative'
      style={{ height: layout.height, width: layout.width }}
      data-testid='admin-app-map-diagram'
    >
      <Edges map={map} layout={layout} lit={lit} litTargets={litTargets} dimmed={!!active} />
      <ColumnHeadings layout={layout} />
      {layout.headings.map((h) => (
        <div
          key={h.group}
          aria-hidden
          className={cn("absolute truncate", HEADING)}
          style={{ left: h.x, top: h.y, width: layout.columns.target.width }}
        >
          {h.label}
        </div>
      ))}
      {map.entries.map((entry) => (
        <EntryNode key={entry.id} entry={entry} box={layout.boxes.get(entry.id)} />
      ))}
      {map.functions.map((fn) => (
        <FunctionNode
          key={fn.id}
          fn={fn}
          box={layout.boxes.get(fn.id)}
          open={selected === fn.name}
          onOpen={() => onOpenFunction(fn.name)}
          onHover={(on) => setHovered(on ? fn.id : null)}
        />
      ))}
      {map.targets.map((target) => (
        <TargetNode
          key={target.id}
          target={target}
          box={layout.boxes.get(target.id)}
          dim={!!active && !litTargets.has(target.id)}
        />
      ))}
    </div>
  );
};

const Edges = ({
  map,
  layout,
  lit,
  litTargets,
  dimmed
}: {
  map: AppMap;
  layout: AppMapLayout;
  lit: (edge: MapEdge) => boolean;
  litTargets: Set<string>;
  dimmed: boolean;
}) => (
  <svg
    aria-hidden
    className='pointer-events-none absolute inset-0 overflow-visible'
    width={layout.width}
    height={layout.height}
  >
    {map.edges.map((edge) => {
      const d = layout.paths.get(edge.id);
      if (!d) return null;
      const failed = edge.failedCalls > 0;
      return (
        <path
          key={edge.id}
          d={d}
          fill='none'
          stroke='currentColor'
          strokeWidth={failed ? 1.5 : 1}
          // A manifest destination no recorded run wrote to: declared, not seen.
          strokeDasharray={edge.kind === "touch" && !edge.observed ? "4 3" : undefined}
          className={cn(
            "transition-opacity",
            failed ? DANGER.text : "text-muted-foreground/50",
            !lit(edge) && "opacity-15"
          )}
          data-testid={`admin-app-map-edge-${edge.id}`}
        />
      );
    })}
    {layout.labels.map((label) => (
      <text
        key={label.target}
        x={label.x}
        y={label.y}
        textAnchor='end'
        strokeWidth={3}
        strokeLinejoin='round'
        paintOrder='stroke'
        className={cn(
          // The card-coloured stroke is a halo, so a label stays legible over the
          // edges it sits on.
          "fill-current stroke-card text-xs transition-opacity",
          label.failed ? DANGER.text : "text-muted-foreground",
          dimmed && !litTargets.has(label.target) && "opacity-15"
        )}
      >
        {label.text}
      </text>
    ))}
  </svg>
);

const ColumnHeadings = ({ layout }: { layout: AppMapLayout }) => (
  <>
    {(
      [
        ["entry", "Starts it"],
        ["fn", "Functions"],
        ["target", "Touches"]
      ] as const
    ).map(([column, text]) => (
      <div
        key={column}
        aria-hidden
        className={cn("absolute top-0 truncate", HEADING)}
        style={{ left: layout.columns[column].x, width: layout.columns[column].width }}
      >
        {text}
      </div>
    ))}
  </>
);

const NODE = "absolute flex items-center gap-1.5 rounded-sm px-1.5 text-xs";

/** What an entry node spells out at console width. A route's path is its function's
 *  name, which sits beside it, and a schedule's clock says "schedule": both give the
 *  room to what tells two entries apart. The full words are the node's title. */
const shown = (entry: MapEntry): { label: string | null; detail: string | null } => {
  if (entry.kind === "route") return { label: entry.label, detail: null };
  if (entry.kind === "schedule") return { label: null, detail: entry.detail };
  return { label: entry.label, detail: entry.detail };
};

const EntryNode = ({ entry, box }: { entry: MapEntry; box: Box | undefined }) => {
  const Icon = ENTRY_ICON[entry.kind];
  const { label, detail } = shown(entry);
  return (
    <div
      aria-hidden
      className={cn(NODE, "border border-border/60 bg-muted/40")}
      style={place(box)}
      title={entryText(entry)}
      data-testid={`admin-app-map-entry-${entry.fn}-${entry.kind}`}
    >
      <Icon className='size-3 shrink-0 text-muted-foreground' />
      {label ? <span className='shrink-0'>{label}</span> : null}
      {detail ? (
        <span className='min-w-0 truncate font-mono text-muted-foreground'>{detail}</span>
      ) : null}
    </div>
  );
};

const FunctionNode = ({
  fn,
  box,
  open,
  onOpen,
  onHover
}: {
  fn: MapFunction;
  box: Box | undefined;
  open: boolean;
  onOpen: () => void;
  onHover: (on: boolean) => void;
}) => (
  <button
    type='button'
    onClick={onOpen}
    onMouseEnter={() => onHover(true)}
    onMouseLeave={() => onHover(false)}
    onFocus={() => onHover(true)}
    onBlur={() => onHover(false)}
    aria-label={`${fn.name}: open in Functions`}
    aria-current={open || undefined}
    className={cn(
      NODE,
      "border bg-card text-left hover:border-foreground/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
      open ? "border-foreground" : "border-border"
    )}
    style={place(box)}
    data-testid={`admin-app-map-function-${fn.name}`}
  >
    <span className='min-w-0 flex-1 truncate font-mono font-medium'>{fn.name}</span>
    {runsLabel(fn, true) ? (
      <span className='shrink-0 text-muted-foreground tabular-nums'>{runsLabel(fn, true)}</span>
    ) : null}
  </button>
);

const TargetNode = ({
  target,
  box,
  dim
}: {
  target: MapTarget;
  box: Box | undefined;
  dim: boolean;
}) => {
  const failed = target.failedCalls > 0;
  return (
    <div
      aria-hidden
      className={cn(
        NODE,
        "transition-opacity",
        failed
          ? cn("ring-1 ring-inset", DANGER.bg, DANGER.ring, DANGER.text)
          : cn("border bg-card", target.observed ? "border-border" : "border-dashed"),
        dim && "opacity-40"
      )}
      style={place(box)}
      title={targetSummary(target)}
      data-testid={`admin-app-map-target-${target.id}`}
    >
      <span className='min-w-0 flex-1 truncate font-mono'>{target.label}</span>
      {target.calls > 0 ? (
        <span className={cn("shrink-0 tabular-nums", !failed && "text-muted-foreground")}>
          {failed ? `${target.failedCalls}/${target.calls}` : target.calls}
        </span>
      ) : null}
    </div>
  );
};
