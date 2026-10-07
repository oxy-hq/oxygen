import type { ReactNode } from "react";
import { Table, TableBody, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";

export interface TokenListColumn {
  /** The heading. Also the column's key, so no two columns share one. */
  label: string;
  /** The column's width. One column goes without and takes what the others leave. */
  className?: string;
  align?: "left" | "right";
  /** The heading is for a screen reader only: the column of row actions. */
  srOnly?: boolean;
}

interface Props {
  /** `admin-<area>`: the table is `<area>-table`, the note under it `<area>-truncated`. */
  area: string;
  columns: TokenListColumn[];
  /** The table's minimum width: under it the table scrolls sideways instead of crushing a cell. */
  className?: string;
  /** How many tokens the server sent. */
  fetched: number;
  /** The most it sends. The response does not say it stopped there, so a list this long is cut. */
  limit: number;
  /** The rows. */
  children: ReactNode;
}

/**
 * The frame of a staff token list: a fixed layout, so a long cell is cut with an ellipsis and
 * Revoke is never scrolled out of sight, and a note when the server's limit cut the list.
 */
export function TokenListTable({ area, columns, className, fetched, limit, children }: Props) {
  return (
    <div className='space-y-2'>
      <div className='overflow-x-auto rounded-md border border-border/60'>
        <Table className={cn("table-fixed text-xs", className)} data-testid={`${area}-table`}>
          <TableHeader>
            <TableRow className={ADMIN_HEADER_ROW_CLASS}>
              {columns.map((column) => (
                <AdminTh key={column.label} align={column.align} className={column.className}>
                  {column.srOnly ? <span className='sr-only'>{column.label}</span> : column.label}
                </AdminTh>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>{children}</TableBody>
        </Table>
      </div>
      {fetched >= limit && (
        <p className='text-muted-foreground text-xs' data-testid={`${area}-truncated`}>
          Showing the newest {limit}. Older tokens are not listed.
        </p>
      )}
    </div>
  );
}
