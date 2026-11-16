import { formatDistanceToNow } from "date-fns";
import { AppWindow, KeyRound } from "lucide-react";
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
import type { SecretsScope, UnifiedRow } from "../types";
import { SharedWith } from "./AvailableTo";
import { RowActions } from "./RowActions";

interface SecretsTableProps {
  scope: SecretsScope;
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
  /** Lift an app's own key into a workspace secret several apps can read. */
  onLift: (row: UnifiedRow) => void;
}

const EMPTY: Record<SecretsScope, { title: string; description: string }> = {
  workspace: {
    title: "No workspace secrets yet",
    description:
      "Store a credential once here and choose which apps read it. Rotating it reaches all of them."
  },
  app: {
    title: "No app has its own secret",
    description:
      "An app stores its own value of a key when it must differ from the workspace's. Most apps read a shared workspace secret instead."
  }
};

/**
 * The rows of one tab.
 *
 * The two tabs differ in their second column, which is the only column that
 * carries a decision: a workspace secret's is the apps that read it, and it is
 * editable in place; an app secret's is simply which app owns it.
 */
export const SecretsTable = ({
  scope,
  rows,
  total,
  loading,
  error,
  onRetry,
  onOpen,
  onShare,
  onEdit,
  onDelete,
  onLift
}: SecretsTableProps) => (
  <TableWrapper>
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>{scope === "app" ? "App" : "Read by"}</TableHead>
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
          noFoundTitle={total === 0 ? EMPTY[scope].title : "No secret matches"}
          noFoundDescription={
            total === 0 ? EMPTY[scope].description : "Try another name, or clear the filter."
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
              <TableCell data-label={scope === "app" ? "App" : "Read by"}>
                {row.app ? (
                  <span className='flex items-center gap-1.5 text-sm'>
                    <AppWindow className='size-3.5 text-muted-foreground' />
                    {row.app.name}
                  </span>
                ) : (
                  <SharedWith row={row} onEdit={onShare} />
                )}
              </TableCell>
              <TableCell
                data-label='Updated'
                className='text-muted-foreground text-xs max-md:hidden'
              >
                {formatDistanceToNow(new Date(row.secretInfo.updated_at), { addSuffix: true })}
              </TableCell>
              <TableCell className='w-px'>
                <RowActions
                  row={row}
                  onShare={onShare}
                  onEdit={onEdit}
                  onDelete={onDelete}
                  onLift={onLift}
                />
              </TableCell>
            </TableRow>
          ))}
        </TableContentWrapper>
      </TableBody>
    </Table>
  </TableWrapper>
);
