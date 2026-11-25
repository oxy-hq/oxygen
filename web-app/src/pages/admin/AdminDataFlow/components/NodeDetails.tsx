import { ArrowLeft, ArrowRight, ChevronsDownUp, Crosshair } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import ROUTES from "@/libs/utils/routes";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { DataFlowEdge, DataFlowNode } from "@/types/dataFlow";
import {
  captionOf,
  describeEdge,
  EDGE_KIND_WORDS,
  type Flow,
  flagsOf,
  neighboursOf
} from "../dataFlowModel";
import type { ViewEdge } from "../dataFlowView";

const EdgeRow = ({
  edge,
  other,
  direction,
  onSelect
}: {
  edge: DataFlowEdge & Pick<ViewEdge, "count">;
  other: DataFlowNode | undefined;
  direction: "in" | "out";
  onSelect: (id: string) => void;
}) => {
  const Arrow = direction === "in" ? ArrowLeft : ArrowRight;
  const otherId = direction === "in" ? edge.from : edge.to;
  return (
    <li className='flex flex-col gap-0.5 py-1.5' data-testid='admin-data-flow-details-edge'>
      <div className='flex items-center gap-1.5'>
        <Arrow className='size-3 shrink-0 text-muted-foreground' />
        <span className='shrink-0 text-muted-foreground'>
          {EDGE_KIND_WORDS[direction][edge.kind]}
        </span>
        <button
          type='button'
          className='min-w-0 truncate font-medium text-foreground hover:underline'
          onClick={() => onSelect(otherId)}
          title={otherId}
        >
          {other?.label ?? otherId}
        </button>
      </div>
      <p
        className={
          edge.approved === false || (edge.problems?.length ?? 0) > 0
            ? `pl-4.5 ${ADMIN_TONE.warn.text}`
            : "pl-4.5 text-muted-foreground"
        }
      >
        {edge.count && edge.count > 1 ? `${edge.count} edges · ` : ""}
        {describeEdge(edge)}
      </p>
    </li>
  );
};

/** The details column before anything is selected: it keeps its width, so the
 *  canvas never resizes (and shifts what was clicked) when a node is picked. */
export const NodeDetailsEmpty = () => (
  <aside
    className='flex w-72 shrink-0 flex-col border-border/60 border-l p-3 text-muted-foreground text-xs'
    data-testid='admin-data-flow-details-empty'
  >
    Click a node to list its edges and what declared each one.
  </aside>
);

/**
 * The selected node: what it is, what is wrong with its declaration, and every
 * edge into and out of it in words, each naming what declared it. "Focus" asks the
 * server for this node's neighbourhood; an app links to its console.
 */
export const NodeDetails = ({
  flow,
  node,
  orgSlug,
  focused,
  onFocus,
  onSelect,
  onCollapse
}: {
  flow: Flow;
  node: DataFlowNode;
  orgSlug: string;
  focused: boolean;
  onFocus: (id: string) => void;
  onSelect: (id: string) => void;
  /** Fold the group this node was opened out of, when it was opened by hand. */
  onCollapse?: () => void;
}) => {
  const byId = new Map(flow.nodes.map((n) => [n.id, n] as const));
  const { incoming, outgoing } = neighboursOf(flow, node.id);
  const flags = flagsOf(node);
  const slug = typeof node.meta.slug === "string" ? node.meta.slug : undefined;
  return (
    <aside
      className='flex w-72 shrink-0 flex-col gap-3 border-border/60 border-l p-3 text-xs'
      data-testid='admin-data-flow-details'
    >
      <header className='flex flex-col gap-0.5'>
        <span className='text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
          {node.type}
        </span>
        <h3 className='break-words font-semibold text-sm'>{node.label}</h3>
        <span className='text-muted-foreground'>{captionOf(node)}</span>
      </header>
      {flags.map((flag) => (
        <p key={flag.label} className={ADMIN_TONE[flag.tone].text}>
          {flag.hint}
        </p>
      ))}
      <div className='flex flex-wrap gap-1.5'>
        {!focused && (
          <Button size='sm' variant='outline' onClick={() => onFocus(node.id)}>
            <Crosshair className='size-3' /> Focus
          </Button>
        )}
        {onCollapse && (
          <Button
            size='sm'
            variant='outline'
            onClick={onCollapse}
            data-testid='admin-data-flow-collapse-group'
          >
            <ChevronsDownUp className='size-3' /> Collapse group
          </Button>
        )}
        {node.type === "app" && slug && (
          <Button size='sm' variant='outline' asChild>
            <Link to={`${ROUTES.ADMIN.CUSTOMER_APPS}/${orgSlug}/${slug}`}>Open console</Link>
          </Button>
        )}
      </div>
      {incoming.length + outgoing.length === 0 ? (
        <p className='text-muted-foreground'>No declared edges.</p>
      ) : (
        <ul className='divide-y divide-border/60'>
          {incoming.map((e) => (
            <EdgeRow
              key={`in:${e.kind}:${e.from}`}
              edge={e}
              other={byId.get(e.from)}
              direction='in'
              onSelect={onSelect}
            />
          ))}
          {outgoing.map((e) => (
            <EdgeRow
              key={`out:${e.kind}:${e.to}`}
              edge={e}
              other={byId.get(e.to)}
              direction='out'
              onSelect={onSelect}
            />
          ))}
        </ul>
      )}
      <p className='break-all font-mono text-[10px] text-muted-foreground'>{node.id}</p>
    </aside>
  );
};
