import { Badge } from "@/components/ui/shadcn/badge";
import type { PreviewCompareTable } from "@/types/workspace";

/** `null` reads as "not computed" rather than zero — never collapse the two. */
function Count({ label, value }: { label: string; value: number | null }) {
  return (
    <div className='flex flex-col'>
      <span className='text-muted-foreground'>{label}</span>
      <span className='font-mono'>{value === null ? "—" : value.toLocaleString()}</span>
    </div>
  );
}

/**
 * One table a compare diffed: counts and column changes only — never a row
 * value. `dropped`/`partial`/`preexisting` are independent flags a table can
 * carry together; `skipped_reason` (our own words, never an engine message)
 * explains a `null` count elsewhere in the row.
 */
export default function PreviewCompareTableRow({ table }: { table: PreviewCompareTable }) {
  return (
    <div
      className='flex flex-col gap-2 rounded-md border p-3'
      data-testid={`preview-compare-table-${table.table}`}
    >
      <div className='flex flex-wrap items-center gap-2'>
        <span className='font-medium font-mono text-sm'>{table.table}</span>
        {table.equal && <Badge variant='outline'>Equal</Badge>}
        {table.preexisting && <Badge variant='outline'>Preexisting</Badge>}
        {table.dropped && <Badge variant='destructive'>Dropped</Badge>}
        {table.partial && <Badge variant='outline'>Partial copy</Badge>}
      </div>
      <div className='grid grid-cols-2 gap-x-4 gap-y-1.5 text-xs sm:grid-cols-4'>
        <Count label='Live rows' value={table.live_rows} />
        <Count label='Preview rows' value={table.preview_rows} />
        <Count label='Only in preview' value={table.only_in_preview} />
        <Count label='Only in live' value={table.only_in_live} />
      </div>
      {(table.columns_added.length > 0 ||
        table.columns_removed.length > 0 ||
        table.columns_retyped.length > 0) && (
        <div className='flex flex-col gap-1 border-t pt-1.5 text-xs'>
          {table.columns_added.length > 0 && (
            <p>
              <span className='font-medium'>Columns added:</span>{" "}
              <span className='font-mono'>{table.columns_added.join(", ")}</span>
            </p>
          )}
          {table.columns_removed.length > 0 && (
            <p>
              <span className='font-medium'>Columns removed:</span>{" "}
              <span className='font-mono'>{table.columns_removed.join(", ")}</span>
            </p>
          )}
          {table.columns_retyped.length > 0 && (
            <p>
              <span className='font-medium'>Columns retyped:</span>{" "}
              <span className='font-mono'>
                {table.columns_retyped
                  .map((c) => `${c.column} (${c.live} → ${c.preview})`)
                  .join(", ")}
              </span>
            </p>
          )}
        </div>
      )}
      {table.skipped_reason && (
        <p className='text-muted-foreground text-xs'>{table.skipped_reason}</p>
      )}
    </div>
  );
}
