import { useMemo } from "react";
import CodeView from "@/components/Code/CodeView";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Panel, PanelContent, PanelHeader } from "@/components/ui/panel";
import type { SqlItem } from "@/hooks/analyticsSteps";
import type { ResultTableData } from "@/services/api/analytics";
import { ResultTableBlock } from "./ResultTableBlock";

/** The `result_id` a react `query` call returned (its persisted output), if any. */
export function resultIdOf(toolOutput: string | undefined): string | undefined {
  try {
    const out: unknown = JSON.parse(toolOutput ?? "");
    const id = (out as { result_id?: unknown } | null)?.result_id;
    return typeof id === "string" ? id : undefined;
  } catch {
    return undefined;
  }
}

/** `number` when every non-null preview cell is a number, else `text`. */
function kindsOf(columns: string[], rows: unknown[][]): ("number" | "text")[] {
  return columns.map((_, i) =>
    rows.every((r) => r[i] === null || r[i] === undefined || typeof r[i] === "number")
      ? "number"
      : "text"
  );
}

/** The stored result behind a react query, as the table pager reads it. */
export function previewTable(item: SqlItem, resultId: string): ResultTableData {
  const [columns = [], ...rows] = (item.result ?? []) as unknown[][];
  const names = columns.map(String);
  return {
    result_id: resultId,
    columns: names,
    column_kinds: kindsOf(names, rows),
    rows,
    row_count: item.rowCount ?? rows.length,
    truncated: false
  };
}

/**
 * A react run's query: its SQL, then every row it returned, paged from the stored
 * result. The event row carries only the first rows; when the result was not stored,
 * the table shows those and says so.
 */
export function ReactQueryPanel({
  item,
  resultId,
  runId,
  onClose
}: {
  item: SqlItem;
  resultId: string;
  runId: string;
  onClose: () => void;
}) {
  const label = item.source === "semantic" ? "Semantic Query" : "SQL Query";
  const subtitle = `${item.rowCount ?? 0} rows · ${item.durationMs ?? 0}ms`;
  // A stored result never changes once written, so its table is fixed per result. The
  // step builder hands a new `item` on every streamed event; keying on it would refetch
  // the page each time.
  // biome-ignore lint/correctness/useExhaustiveDependencies: fixed per result id, see above
  const table = useMemo(() => previewTable(item, resultId), [resultId]);
  const sqlHeight = Math.min(Math.max(item.sql.split("\n").length * 18 + 24, 120), 320);
  return (
    <Panel>
      <PanelHeader
        title={<h3 className='font-semibold text-sm'>{label}</h3>}
        subtitle={subtitle}
        onClose={onClose}
      />
      <PanelContent scrollable={true} padding={false} className='flex min-h-0 flex-col'>
        <div style={{ height: sqlHeight }}>
          <CodeView value={item.sql} language='sql' />
        </div>
        {item.error ? (
          <ErrorAlert className='mx-3 my-2 max-h-32 overflow-y-auto' message={item.error} />
        ) : (
          <div className='border-t p-3' data-testid='react-query-results'>
            {/* Keyed by result: picking another query starts at its first page, unsorted. */}
            <ResultTableBlock key={`${runId}:${resultId}`} runId={runId} table={table} />
          </div>
        )}
      </PanelContent>
    </Panel>
  );
}
