import { Table, TableBody, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import type { AuditEvent } from "@/types/audit";
import { AuditRow } from "./AuditRow";

interface Props {
  events: AuditEvent[];
  limit: number;
  /** The token the log is narrowed to, if any: its rows do not offer to narrow to it again. */
  tokenId?: string;
  /** Narrow the log to one token. */
  onFilterToken?: (tokenId: string) => void;
}

/**
 * The loaded stream. Loading, failure and "nothing matched" are no longer this
 * component's business — the page gates all three through `AdminAsync`, which
 * is also where the retry the old inline error block never offered now lives.
 * So `events` arrives already resolved and non-empty.
 *
 * The columns are the six the table always had, behind a narrow one for the chevron that opens
 * a row's credential and client. Those are long, so they get the row's width, not a column.
 */
export default function AuditTable({ events, limit, tokenId, onFilterToken }: Props) {
  return (
    <div className='space-y-2'>
      <div className='overflow-x-auto rounded-md border border-border/60'>
        <Table className='text-xs'>
          <TableHeader>
            <TableRow className={ADMIN_HEADER_ROW_CLASS}>
              <AdminTh className='w-6 pr-0'>
                <span className='sr-only'>Details</span>
              </AdminTh>
              <AdminTh>When</AdminTh>
              <AdminTh>Actor</AdminTh>
              <AdminTh>Action</AdminTh>
              <AdminTh>Target</AdminTh>
              <AdminTh>Scope</AdminTh>
              <AdminTh align='right'>Outcome</AdminTh>
            </TableRow>
          </TableHeader>
          <TableBody>
            {events.map((event) => (
              <AuditRow
                key={event.id}
                event={event}
                onFilterToken={
                  event.token_id && event.token_id === tokenId ? undefined : onFilterToken
                }
              />
            ))}
          </TableBody>
        </Table>
      </div>
      {events.length >= limit && (
        <p className='text-muted-foreground text-xs'>
          Showing the most recent {limit}. Narrow the filters to reach older events.
        </p>
      )}
    </div>
  );
}
