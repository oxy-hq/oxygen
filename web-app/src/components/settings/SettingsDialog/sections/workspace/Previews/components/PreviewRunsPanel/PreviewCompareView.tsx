import type { PreviewCompare } from "@/types/workspace";
import PreviewCompareTableRow from "./PreviewCompareTableRow";
import PreviewRunStateBadge from "./PreviewRunStateBadge";

/**
 * A `transform_build`'s linked compare, or a `compare` run's own detail.
 * Caveats come first and prominent — they qualify every count below them
 * (e.g. live may be staler or fresher than the build) — then each table.
 * `tables` is `[]` until the compare succeeds, same shape as an empty steps
 * list elsewhere in this panel.
 */
export default function PreviewCompareView({ compare }: { compare: PreviewCompare }) {
  return (
    <div className='flex flex-col gap-2' data-testid={`preview-compare-${compare.run_id}`}>
      <div className='flex flex-wrap items-center gap-2'>
        <PreviewRunStateBadge state={compare.state} outcome={compare.outcome} />
        {compare.error && <p className='text-destructive text-xs'>{compare.error}</p>}
      </div>
      {compare.caveats.length > 0 && (
        <ul
          className='flex flex-col gap-1 rounded-md border border-warning/40 bg-warning/10 p-2 text-warning text-xs'
          data-testid='preview-compare-caveats'
        >
          {compare.caveats.map((caveat, i) => (
            // Caveats are fixed sentences with no id of their own.
            // biome-ignore lint/suspicious/noArrayIndexKey: caveats have no stable id
            <li key={i}>{caveat}</li>
          ))}
        </ul>
      )}
      {compare.tables.length === 0 ? (
        <p className='text-muted-foreground text-xs'>
          {compare.state === "finished" ? "No tables compared." : "Comparing…"}
        </p>
      ) : (
        <div className='flex flex-col gap-2'>
          {compare.tables.map((table) => (
            <PreviewCompareTableRow key={table.table} table={table} />
          ))}
        </div>
      )}
    </div>
  );
}
