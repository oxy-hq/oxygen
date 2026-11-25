import { ArrowDown, ArrowUp, ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { extractResultCharts, extractResultTables, type SseEvent } from "@/hooks/useAnalyticsRun";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import { AnalyticsService, type ResultTableData } from "@/services/api/analytics";
import { ResultChartBlock } from "./ResultChartBlock";

/** Rows per page. The answer's row carries the first page; the rest are fetched. */
export const PAGE_ROWS = 50;

/** The sorted column by its place: a join can return two columns with one name. */
type Sort = { column: number; desc: boolean } | null;
type Page = { rows: unknown[][]; notice?: string };

/** A cell as text: numbers keep every digit they arrived with, nulls are a dash. */
export function cellText(value: unknown, kind: "number" | "text"): string {
  if (value === null || value === undefined) return "—";
  if (kind === "number" && typeof value === "number") {
    return value.toLocaleString(undefined, { maximumFractionDigits: 20 });
  }
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** Why a page could not be fetched, in words: the stored result is gone or expired. */
function pageNotice(err: unknown): string {
  const status = (err as { response?: { status?: number } })?.response?.status;
  if (status === 410) return "This result expired after 30 days, so only its first rows remain.";
  if (status === 404) return "The full result was not stored, so only its first rows are shown.";
  return "Could not load these rows.";
}

/**
 * A react answer's result: the rows the query returned, as returned. The first
 * page comes with the answer; later pages and sorting are fetched from the
 * stored result, sorted on the server so every page is from one order.
 */
export function ResultTableBlock({ runId, table }: { runId: string; table: ResultTableData }) {
  const { project } = useCurrentProjectBranch();
  const [offset, setOffset] = useState(0);
  const [sort, setSort] = useState<Sort>(null);
  const local = offset === 0 && sort === null;
  const [page, setPage] = useState<Page>({ rows: table.rows });
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (local) {
      setPage({ rows: table.rows });
      return;
    }
    let live = true;
    setLoading(true);
    AnalyticsService.getResultPage(project.id, runId, table.result_id, {
      offset,
      limit: PAGE_ROWS,
      sort: sort?.column,
      desc: sort?.desc
    })
      .then((p) => live && setPage({ rows: p.rows }))
      .catch((err) => live && setPage({ rows: [], notice: pageNotice(err) }))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
  }, [local, offset, sort, project.id, runId, table.result_id, table.rows]);

  const toggleSort = (column: number) => {
    setOffset(0);
    setSort((s) =>
      s?.column !== column ? { column, desc: false } : s.desc ? null : { column, desc: true }
    );
  };
  const total = table.row_count;
  const shownTo = Math.min(offset + page.rows.length, total);
  const more = table.truncated ? " (the query had more rows than were kept)" : "";

  return (
    <div className='flex flex-col gap-2' data-testid='react-result-table'>
      <Table containerClassName='max-h-[480px] overflow-auto rounded-md border'>
        <TableHeader className='sticky top-0 bg-background'>
          <TableRow>
            {table.columns.map((col, c) => (
              <TableHead
                // biome-ignore lint/suspicious/noArrayIndexKey: names can repeat; a column is its place
                key={c}
                className={cn(
                  "cursor-pointer select-none whitespace-nowrap",
                  table.column_kinds[c] === "number" && "text-right"
                )}
                onClick={() => toggleSort(c)}
              >
                {col}
                {sort?.column === c &&
                  (sort.desc ? (
                    <ArrowDown className='ml-1 inline h-3 w-3' />
                  ) : (
                    <ArrowUp className='ml-1 inline h-3 w-3' />
                  ))}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody className={cn(loading && "opacity-50")}>
          {page.rows.map((row, r) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: a row's identity is its place in the result
            <TableRow key={offset + r}>
              {table.columns.map((_col, c) => {
                const kind = table.column_kinds[c] ?? "text";
                return (
                  <TableCell
                    // biome-ignore lint/suspicious/noArrayIndexKey: names can repeat; a column is its place
                    key={c}
                    className={cn(
                      "whitespace-nowrap",
                      kind === "number" && "text-right tabular-nums",
                      (row[c] === null || row[c] === undefined) && "text-muted-foreground"
                    )}
                  >
                    {cellText(row[c], kind)}
                  </TableCell>
                );
              })}
            </TableRow>
          ))}
        </TableBody>
      </Table>
      {page.notice && <p className='text-muted-foreground text-xs'>{page.notice}</p>}
      <div className='flex items-center justify-between text-muted-foreground text-xs'>
        <span>
          {total === 0
            ? "No rows"
            : `Rows ${offset + 1}–${shownTo} of ${total.toLocaleString()}${more}`}
        </span>
        {total > PAGE_ROWS && (
          <div className='flex gap-1'>
            <Button
              variant='ghost'
              size='sm'
              disabled={offset === 0 || loading}
              onClick={() => setOffset((o) => Math.max(0, o - PAGE_ROWS))}
              aria-label='Previous rows'
            >
              <ChevronLeft className='h-4 w-4' />
            </Button>
            <Button
              variant='ghost'
              size='sm'
              disabled={offset + PAGE_ROWS >= total || loading || !!page.notice}
              onClick={() => setOffset((o) => o + PAGE_ROWS)}
              aria-label='Next rows'
            >
              <ChevronRight className='h-4 w-4' />
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

/** A run's charts (`result_chart` rows), then its answer tables (`result_table`), each in order. */
export function RunResults({ events, runId }: { events: SseEvent[]; runId: string }) {
  return (
    <>
      {extractResultCharts(events).map((chart, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: rows only append, and two charts may share a result
        <ResultChartBlock key={i} chart={chart} index={i} />
      ))}
      {extractResultTables(events).map((table) => (
        <ResultTableBlock key={table.result_id} runId={runId} table={table} />
      ))}
    </>
  );
}
