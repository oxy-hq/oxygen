import type { ReactNode } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import type { AuditEvent } from "@/types/audit";
import type { AuditCredential } from "../auditCredential";

const Line = ({
  label,
  testId,
  children
}: {
  label: string;
  testId: string;
  children: ReactNode;
}) => (
  <div className='flex gap-3' data-testid={testId}>
    <dt className='w-20 shrink-0 text-muted-foreground'>{label}</dt>
    <dd className='flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1'>{children}</dd>
  </div>
);

interface Props {
  /** The `id` the row's toggle names in `aria-controls`. */
  id: string;
  event: AuditEvent;
  credential: AuditCredential | null;
  /** How many columns the table has: the detail spans all but the toggle's. */
  columns: number;
  /** Narrow the log to this row's token. Absent when it is already narrowed to it. */
  onFilterToken?: (tokenId: string) => void;
}

/**
 * What a row keeps out of its line: the token it names, the address the request came from and
 * the client that sent it. A user agent is long and is the text being looked for, so it wraps
 * here in full and the table never grows a column for it.
 */
export function AuditDetail({ id, event, credential, columns, onFilterToken }: Props) {
  const tokenId = credential?.id;
  return (
    <TableRow
      id={id}
      className='border-border/50 bg-muted/20 hover:bg-muted/20'
      data-testid={`admin-audit-detail-${event.id}`}
    >
      <TableCell className='py-0' />
      <TableCell colSpan={columns - 1} className='whitespace-normal py-2'>
        <dl className='space-y-1'>
          {credential && (
            <Line
              label={credential.role === "acted" ? "Acted with" : "About token"}
              testId='admin-audit-detail-token'
            >
              <span className='font-medium'>{credential.name ?? "A token with no name"}</span>
              {credential.kind && <span className='text-muted-foreground'>{credential.kind}</span>}
              {credential.prefix && (
                <span className='font-mono text-muted-foreground'>{credential.prefix}</span>
              )}
              {tokenId && onFilterToken && (
                <Button
                  variant='outline'
                  size='sm'
                  // `!`: the button's own size class would set the label larger than the row.
                  className='h-5 px-1.5 font-normal text-xs!'
                  onClick={() => onFilterToken(tokenId)}
                  data-testid='admin-audit-filter-token'
                >
                  Only this token
                </Button>
              )}
            </Line>
          )}
          {event.ip && (
            <Line label='Address' testId='admin-audit-detail-ip'>
              <span className='font-mono'>{event.ip}</span>
            </Line>
          )}
          {event.user_agent && (
            <Line label='Client' testId='admin-audit-detail-client'>
              <span className='break-all font-mono'>{event.user_agent}</span>
            </Line>
          )}
        </dl>
      </TableCell>
    </TableRow>
  );
}
