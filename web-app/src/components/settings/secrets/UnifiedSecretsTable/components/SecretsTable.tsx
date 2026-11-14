import { formatDistanceToNow } from "date-fns";
import { KeyRound } from "lucide-react";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import TableContentWrapper from "../../../components/TableContentWrapper";
import TableWrapper from "../../../components/TableWrapper";
import type { UnifiedRow } from "../types";
import { AvailableTo } from "./AvailableTo";
import { RowActions } from "./RowActions";

/**
 * The rows. Each is a clickable summary — name and description, who can read it,
 * when it last changed — with its verbs one menu away; the value and the rest of
 * the metadata live in the detail dialog. On narrow viewports TableWrapper
 * collapses each row into a stacked card.
 */
export const SecretsTable = ({
  rows,
  total,
  loading,
  error,
  onRetry,
  onOpen,
  onShare,
  onEdit,
  onDelete
}: {
  rows: UnifiedRow[];
  /** Rows before search and filter — tells "nothing stored" from "nothing matches". */
  total: number;
  loading: boolean;
  error?: string;
  onRetry: () => void;
  onOpen: (row: UnifiedRow) => void;
  onShare: (row: UnifiedRow) => void;
  onEdit: (row: UnifiedRow) => void;
  onDelete: (row: UnifiedRow) => void;
}) => (
  <TableWrapper>
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Available to</TableHead>
          <TableHead className='max-md:hidden'>Updated</TableHead>
          <TableHead className='w-px' />
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableContentWrapper
          isEmpty={rows.length === 0}
          loading={loading}
          colSpan={4}
          error={error}
          noFoundTitle={total === 0 ? "No secrets yet" : "No secret matches"}
          noFoundDescription={
            total === 0
              ? "Create one to store a credential once and share it with any of this workspace's apps."
              : "Try another name, or clear the filter."
          }
          onRetry={onRetry}
        >
          {rows.map((row) => (
            <TableRow
              key={row.key}
              className='group cursor-pointer'
              tabIndex={0}
              data-testid={`secret-row-${row.name}`}
              onClick={() => onOpen(row)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onOpen(row);
                }
              }}
            >
              <TableCell data-label='Name'>
                <div className='flex min-w-0 items-start gap-2'>
                  <KeyRound className='mt-0.5 size-3.5 shrink-0 text-muted-foreground/50' />
                  <div className='flex min-w-0 flex-col'>
                    <span className='font-medium font-mono text-sm max-md:whitespace-normal max-md:break-all'>
                      {row.name}
                    </span>
                    {row.secretInfo.description && (
                      <span className='max-w-xs truncate text-muted-foreground text-xs'>
                        {row.secretInfo.description}
                      </span>
                    )}
                  </div>
                </div>
              </TableCell>
              <TableCell data-label='Available to'>
                <AvailableTo row={row} />
              </TableCell>
              <TableCell
                data-label='Updated'
                className='text-muted-foreground text-xs max-md:hidden'
              >
                {formatDistanceToNow(new Date(row.secretInfo.updated_at), { addSuffix: true })}
              </TableCell>
              <TableCell className='w-px'>
                <RowActions row={row} onShare={onShare} onEdit={onEdit} onDelete={onDelete} />
              </TableCell>
            </TableRow>
          ))}
        </TableContentWrapper>
      </TableBody>
    </Table>
  </TableWrapper>
);
