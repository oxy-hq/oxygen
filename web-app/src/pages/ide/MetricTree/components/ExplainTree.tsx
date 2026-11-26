import type { ExplainNode, SplitKind } from "@/types/metricTree";
import { formatSigned } from "@/utils/measureFormat";

/** Human-readable label for a split — matches airlayer's SplitKind variants. */
export function splitLabel(split: SplitKind): string {
  switch (split.type) {
    case "component":
      return `component · ${split.child_measure}`;
    case "dimension":
      return `${split.dimension} = ${split.value}`;
    case "uniform_degradation":
      return `uniform degradation across ${split.dimension} (${split.num_elements} values)`;
    case "cross_cutting":
      return `cross-cutting · ${split.dimension} = ${split.value}`;
  }
}

/** Recursive node row. Indented by depth × 16px. Exported so the
 *  Insights inbox drawer and the Metric Tree RCA panel share the
 *  exact same rendering. */
export function ExplainNodeRow({
  node,
  depth,
  parentDelta
}: {
  node: ExplainNode;
  depth: number;
  parentDelta: number;
}) {
  const pct =
    Math.abs(parentDelta) > 1e-9
      ? `${((node.delta / parentDelta) * 100).toFixed(1)}% of parent`
      : null;
  return (
    <li>
      <div className='border border-border bg-card p-2 text-sm' style={{ marginLeft: depth * 16 }}>
        <div className='flex flex-wrap items-center justify-between gap-x-2 gap-y-1'>
          <span className='break-all font-medium'>{splitLabel(node.split)}</span>
          {pct && (
            <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>{pct}</span>
          )}
        </div>
        <p className='text-muted-foreground text-xs'>
          {node.measure} · delta {formatSigned(node.delta)}
        </p>
      </div>
      {node.children && node.children.length > 0 && (
        <ul className='mt-1 flex flex-col gap-1'>
          {node.children.map((child, i) => (
            <ExplainNodeRow
              key={`${child.measure}-${i}`}
              node={child}
              depth={depth + 1}
              parentDelta={node.delta}
            />
          ))}
        </ul>
      )}
    </li>
  );
}
