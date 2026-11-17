import { ArrowDown, ArrowUp, ChevronLeft, ChevronRight, Loader2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useAppDataRows } from "@/hooks/api/customApps/useAppData";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { AppDataColumn, AppDataOrderTerm, AppDataRows, AppDataTable } from "@/types/appData";
import {
  cellView,
  headerTitle,
  narrowedNote,
  nextOrder,
  pageRange,
  pageRows
} from "./dataBrowserModel";

/** The page size asked for; the server reads fewer for a wide table and says so. */
const PAGE_SIZE = 50;

/**
 * One table's rows, a page at a time: a sticky header, values in monospace, `NULL`
 * drawn apart from any string. A header orders by its column (ascending, descending,
 * then the table's own order); the server checks the name against the table.
 */
export const RowsGrid = ({
  appId,
  environment,
  table,
  textChars
}: {
  appId: string;
  environment: string;
  table: AppDataTable;
  textChars: number;
}) => {
  const [offset, setOffset] = useState(0);
  const [order, setOrder] = useState<AppDataOrderTerm | undefined>(undefined);
  const rows = useAppDataRows(appId, environment, {
    table: table.name,
    limit: PAGE_SIZE,
    offset,
    order
  });
  const sortBy = (column: string) => {
    setOrder(nextOrder(order, column));
    setOffset(0);
  };

  // A page that failed takes the pager and the headers with it, and Retry asks for the
  // same page again — a sort on an unindexed column that timed out would time out
  // again. So a failed page that is not the default one offers the way back to it.
  const strayed = rows.isError && (offset > 0 || order !== undefined);

  return (
    <div className='flex min-w-0 flex-col gap-1.5' data-testid='admin-app-data-rows'>
      {strayed ? (
        <Button
          variant='ghost'
          size='sm'
          className='h-7 self-start text-xs'
          onClick={() => {
            setOrder(undefined);
            setOffset(0);
          }}
          data-testid='admin-app-data-reset'
        >
          Back to the first page, in the table&rsquo;s own order
        </Button>
      ) : null}
      <AdminAsync query={rows} noun={`rows of ${table.name}`} rows={4}>
        {(page) => (
          <>
            <Pager
              page={page}
              loading={rows.isFetching}
              onPage={(next) => setOffset(Math.max(0, next))}
            />
            <Grid page={page} order={order} onSort={sortBy} textChars={textChars} />
          </>
        )}
      </AdminAsync>
    </div>
  );
};

const Pager = ({
  page,
  loading,
  onPage
}: {
  page: AppDataRows;
  loading: boolean;
  onPage: (offset: number) => void;
}) => {
  const narrowed = narrowedNote(page);
  return (
    <div className='flex flex-wrap items-center justify-between gap-2 text-xs'>
      <span className='flex min-w-0 items-center gap-1.5 text-muted-foreground'>
        <span className='truncate font-mono text-foreground'>{page.table}</span>
        <span data-testid='admin-app-data-range'>{pageRange(page.offset, page.rows.length)}</span>
        {loading ? <Loader2 className='size-3 animate-spin' /> : null}
      </span>
      <span className='flex items-center gap-1'>
        {narrowed ? (
          <span className='text-muted-foreground' data-testid='admin-app-data-narrowed'>
            {narrowed}
          </span>
        ) : null}
        <Button
          variant='ghost'
          size='sm'
          className='h-7 px-1.5 text-xs'
          disabled={page.offset === 0 || loading}
          onClick={() => onPage(page.offset - page.limit)}
          aria-label='Previous page'
          data-testid='admin-app-data-prev'
        >
          <ChevronLeft className='size-3' />
        </Button>
        <Button
          variant='ghost'
          size='sm'
          className='h-7 px-1.5 text-xs'
          disabled={!page.has_more || loading}
          onClick={() => onPage(page.offset + page.limit)}
          aria-label='Next page'
          data-testid='admin-app-data-next'
        >
          <ChevronRight className='size-3' />
        </Button>
      </span>
    </div>
  );
};

const Grid = ({
  page,
  order,
  onSort,
  textChars
}: {
  page: AppDataRows;
  order: AppDataOrderTerm | undefined;
  onSort: (column: string) => void;
  textChars: number;
}) => (
  <div className='max-h-[28rem] overflow-auto rounded-md border border-border/60'>
    <table className='w-max min-w-full border-collapse text-xs' data-testid='admin-app-data-grid'>
      <thead className='sticky top-0 z-10 bg-card shadow-[0_1px_0_var(--border)]'>
        <tr>
          {page.columns.map((column) => (
            <HeaderCell
              key={column.name}
              column={column}
              dir={order?.column === column.name ? order.dir : undefined}
              onSort={() => onSort(column.name)}
            />
          ))}
        </tr>
      </thead>
      <tbody>
        {page.rows.length === 0 ? (
          <tr>
            <td
              colSpan={Math.max(1, page.columns.length)}
              className='px-2 py-6 text-center text-muted-foreground'
              data-testid='admin-app-data-empty'
            >
              {pageRange(page.offset, 0)}
            </td>
          </tr>
        ) : (
          pageRows(page).map(({ position, cells }) => (
            <tr key={position} className='border-border/40 border-t hover:bg-muted/40'>
              {cells.map(({ column, cell }) => {
                const view = cellView(cell, textChars);
                return (
                  <td
                    key={column}
                    title={view.title}
                    data-null={view.isNull || undefined}
                    className={cn(
                      "max-w-[24rem] truncate whitespace-nowrap px-2 py-1 align-top",
                      view.isNull ? "text-muted-foreground/70 italic" : "font-mono"
                    )}
                  >
                    {view.text}
                  </td>
                );
              })}
            </tr>
          ))
        )}
      </tbody>
    </table>
  </div>
);

const HeaderCell = ({
  column,
  dir,
  onSort
}: {
  column: AppDataColumn;
  dir: "asc" | "desc" | undefined;
  onSort: () => void;
}) => {
  const label = (
    <>
      <span className='font-mono'>{column.name}</span>
      <span className='font-normal text-[10px] text-muted-foreground'>{column.type}</span>
    </>
  );
  return (
    <th
      scope='col'
      aria-sort={dir === "asc" ? "ascending" : dir === "desc" ? "descending" : undefined}
      className='whitespace-nowrap px-2 py-1 text-left font-medium'
    >
      {column.sortable ? (
        <button
          type='button'
          onClick={onSort}
          className='flex items-baseline gap-1.5 hover:text-foreground'
          title={headerTitle(column)}
          data-testid={`admin-app-data-col-${column.name}`}
        >
          {label}
          {dir === "asc" ? <ArrowUp className='size-3 self-center' /> : null}
          {dir === "desc" ? <ArrowDown className='size-3 self-center' /> : null}
        </button>
      ) : (
        // No button: Postgres has no ordering for this type, so a sort could only fail.
        <span
          className='flex items-baseline gap-1.5'
          title={headerTitle(column)}
          data-testid={`admin-app-data-col-${column.name}`}
        >
          {label}
        </span>
      )}
    </th>
  );
};
