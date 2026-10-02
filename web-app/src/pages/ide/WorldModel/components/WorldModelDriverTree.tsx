import { useWmMeasureBreakdown } from "@/hooks/api/useWorldModel";
import { cn } from "@/libs/shadcn/utils";
import type { WmBreakdownEdge, WmBreakdownNode, WmMeasureBreakdown } from "@/types/worldModel";
import { measureSymbol, measureSymbolColor, OP_GLYPH } from "../worldModelLayout";

function formatMeasureValue(raw: string): string {
  const n = Number(raw);
  if (!Number.isFinite(n)) return raw;
  if (Number.isInteger(n)) return n.toLocaleString();
  const formatted = n.toPrecision(7).replace(/\.?0+$/, "");
  return Number(formatted).toLocaleString(undefined, { maximumFractionDigits: 4 });
}

/** Component edges point child(from) → parent(to); children of `nodeId`
 *  are the edges whose `to` equals it. */
function childrenOf(edges: WmBreakdownEdge[], nodeId: string): WmBreakdownEdge[] {
  return edges.filter((e) => e.to === nodeId);
}

function NodeValue({ node }: { node: WmBreakdownNode }) {
  if (node.unvalued_reason) {
    return (
      <span
        className='shrink-0 font-mono text-[9px] text-muted-foreground italic'
        title={node.unvalued_reason}
      >
        {node.unvalued_reason}
      </span>
    );
  }
  if (node.value === null) {
    return <span className='h-3 w-12 shrink-0 animate-pulse rounded bg-muted' />;
  }
  return (
    <span className='shrink-0 font-mono text-[12px] text-info tabular-nums' title={node.value}>
      {formatMeasureValue(node.value)}
    </span>
  );
}

function DriverRow({
  node,
  nodes,
  edges,
  depth
}: {
  node: WmBreakdownNode;
  nodes: WmBreakdownNode[];
  edges: WmBreakdownEdge[];
  depth: number;
}) {
  const kids = childrenOf(edges, node.id);

  return (
    <div className={cn("flex flex-col gap-1", depth > 0 && "border-info/20 border-l pl-2")}>
      <div className='flex min-w-0 items-baseline justify-between gap-2'>
        <div className='flex min-w-0 items-baseline gap-1.5'>
          <span
            className={cn(
              "w-3 shrink-0 text-center font-mono text-[11px] leading-none",
              measureSymbolColor(node.measure_type)
            )}
          >
            {measureSymbol(node.measure_type)}
          </span>
          <span className='min-w-0 truncate text-[11.5px] text-foreground'>{node.label}</span>
        </div>
        <NodeValue node={node} />
      </div>
      {kids.map((edge) => {
        const child = nodes.find((n) => n.id === edge.from);
        if (!child) return null;
        return (
          <div key={edge.from} className='flex items-start gap-1'>
            <span
              className='w-3 shrink-0 pt-0.5 text-center font-mono text-[12px] text-muted-foreground'
              title={edge.operator}
            >
              {OP_GLYPH[edge.operator]}
            </span>
            <div className='min-w-0 flex-1'>
              <DriverRow node={child} nodes={nodes} edges={edges} depth={depth + 1} />
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** Render an already-assembled breakdown tree. */
function WorldModelDriverTree({ breakdown }: { breakdown: WmMeasureBreakdown }) {
  const root = breakdown.nodes.find((n) => n.id === breakdown.root);
  if (!root) return null;
  return <DriverRow node={root} nodes={breakdown.nodes} edges={breakdown.edges} depth={0} />;
}

function TreeMessage({ children, error = false }: { children: React.ReactNode; error?: boolean }) {
  return (
    <div
      className={cn(
        "py-1 font-mono text-[10px]",
        error ? "text-destructive" : "text-muted-foreground"
      )}
    >
      {children}
    </div>
  );
}

/** Stream + render a measure's breakdown valued at a specific instance. */
export function WorldModelDriverTreeLive({
  entityId,
  keyValue,
  measure
}: {
  entityId: string;
  keyValue: string | null;
  measure: string;
}) {
  const { data, isLoading, error } = useWmMeasureBreakdown(entityId, keyValue, measure);

  if (!keyValue) return <TreeMessage>pick an instance to value this breakdown</TreeMessage>;
  // A failed stream leaves at most a partial tree, whose unvalued nodes would pulse forever.
  if (error) return <TreeMessage error>failed to compute breakdown</TreeMessage>;
  if (!data) {
    return (
      <TreeMessage>{isLoading ? "computing breakdown…" : "no breakdown available"}</TreeMessage>
    );
  }
  return <WorldModelDriverTree breakdown={data} />;
}
