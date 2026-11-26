import { cn } from "@/libs/shadcn/utils";
import type { AppDataTable } from "@/types/appData";
import { approxRowsLabel } from "./dataBrowserModel";

/** The schema's tables: name, Postgres's row estimate, and how many columns. */
export const TableList = ({
  schema,
  tables,
  selected,
  onSelect
}: {
  schema: string;
  tables: AppDataTable[];
  selected: string | null;
  onSelect: (table: string) => void;
}) => (
  <div className='flex min-w-0 flex-col gap-1' data-testid='admin-app-data-tables'>
    <span
      className='truncate font-mono text-[10px] text-muted-foreground'
      title={`Schema ${schema}`}
      data-testid='admin-app-data-schema'
    >
      {schema}
    </span>
    <ul className='flex max-h-[28rem] flex-col gap-0.5 overflow-auto'>
      {tables.map((table) => (
        <li key={table.name}>
          <button
            type='button'
            onClick={() => onSelect(table.name)}
            aria-pressed={table.name === selected}
            className={cn(
              "flex w-full min-w-0 items-baseline justify-between gap-2 px-2 py-1 text-left text-xs hover:bg-muted/60",
              table.name === selected && "bg-muted font-medium"
            )}
            data-testid={`admin-app-data-table-${table.name}`}
          >
            <span className='truncate font-mono'>{table.name}</span>
            <span
              className='shrink-0 text-muted-foreground tabular-nums'
              title={`${table.columns.length} column${table.columns.length === 1 ? "" : "s"}`}
            >
              {approxRowsLabel(table.approx_rows)}
            </span>
          </button>
        </li>
      ))}
    </ul>
  </div>
);
