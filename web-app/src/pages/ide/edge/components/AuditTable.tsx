import { Loader2, RefreshCw } from "lucide-react";
import type React from "react";
import { useState } from "react";
import TableContentWrapper from "@/components/settings/components/TableContentWrapper";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import useAuditEvents from "@/hooks/api/cameras/useAuditEvents";
import type { AuditEventRow } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

const ALL_ACTIONS = "__all__";

/** Action filter dropdown — operators usually scan either "all"
 *  or one aggregate. Free-text search isn't worth the extra UI
 *  weight at V1. */
const ACTION_FILTERS: { value: string; label: string }[] = [
  { value: ALL_ACTIONS, label: "All actions" },
  { value: "edge_box.", label: "Edge boxes" },
  { value: "device.", label: "Devices" },
  { value: "pack.", label: "Domain packs" }
];

const AuditTable: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const [actionPrefix, setActionPrefix] = useState<string>(ALL_ACTIONS);
  const {
    data: events = [],
    isLoading,
    isFetching,
    error,
    refetch
  } = useAuditEvents(workspace?.id, {
    action_prefix: actionPrefix === ALL_ACTIONS ? undefined : actionPrefix,
    limit: 200
  });

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex flex-wrap items-end justify-between gap-3'>
        <div className='flex items-center gap-2'>
          <Label
            htmlFor='audit-action'
            className='text-muted-foreground text-xs uppercase tracking-wide'
          >
            Filter
          </Label>
          <Select value={actionPrefix} onValueChange={setActionPrefix}>
            <SelectTrigger id='audit-action' className='h-8 w-44 text-xs'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              {ACTION_FILTERS.map((f) => (
                <SelectItem key={f.value} value={f.value}>
                  {f.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className='text-muted-foreground text-xs'>{events.length} events</span>
        </div>
        <Button
          variant='outline'
          size='sm'
          onClick={() => refetch()}
          disabled={isFetching}
          aria-label='Refresh audit log'
        >
          {isFetching ? (
            <Loader2 className='mr-1.5 h-3.5 w-3.5 animate-spin' />
          ) : (
            <RefreshCw className='mr-1.5 h-3.5 w-3.5' />
          )}
          Refresh
        </Button>
      </div>

      <TableWrapper>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Time</TableHead>
              <TableHead>Action</TableHead>
              <TableHead>Target</TableHead>
              <TableHead>Actor</TableHead>
              <TableHead>Details</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableContentWrapper
              isEmpty={events.length === 0}
              loading={isLoading}
              colSpan={5}
              noFoundTitle='No audit events yet'
              noFoundDescription='Events are recorded when operators register boxes, set targets, or claim devices.'
              error={error?.message}
              onRetry={() => refetch()}
            >
              {events.map((e) => (
                <AuditRow key={e.id} event={e} />
              ))}
            </TableContentWrapper>
          </TableBody>
        </Table>
      </TableWrapper>
    </div>
  );
};

const AuditRow: React.FC<{ event: AuditEventRow }> = ({ event }) => {
  const details = formatDetails(event.details_json);
  return (
    <TableRow>
      <TableCell className='whitespace-nowrap text-muted-foreground text-xs'>
        {new Date(event.created_at).toLocaleString()}
      </TableCell>
      <TableCell>
        <Badge variant='secondary' className='font-mono text-xs'>
          {event.action}
        </Badge>
      </TableCell>
      <TableCell className='font-mono text-muted-foreground text-xs'>
        {event.target_id ? shortId(event.target_id) : "—"}
      </TableCell>
      <TableCell className='font-mono text-muted-foreground text-xs'>
        {event.actor_user_id ? shortId(event.actor_user_id) : "system"}
      </TableCell>
      <TableCell className='text-muted-foreground text-xs'>{details}</TableCell>
    </TableRow>
  );
};

/** Render details_json as a compact `key=value, key=value` string.
 *  Operators scanning the log want a one-liner, not pretty JSON;
 *  the raw object is one click away in DevTools if they need it. */
function formatDetails(details: Record<string, unknown>): string {
  const entries = Object.entries(details).filter(([, v]) => v !== null && v !== undefined);
  if (entries.length === 0) return "—";
  return entries
    .map(([k, v]) => {
      if (typeof v === "string") return `${k}=${v}`;
      if (typeof v === "number" || typeof v === "boolean") return `${k}=${v}`;
      if (Array.isArray(v)) return `${k}=[${v.length}]`;
      return `${k}=…`;
    })
    .join(", ");
}

function shortId(id: string): string {
  if (id.length <= 10) return id;
  return `${id.slice(0, 8)}…`;
}

export default AuditTable;
