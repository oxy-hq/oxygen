import { Loader2, RefreshCw } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Switch } from "@/components/ui/shadcn/switch";
import useEdgeBoxLogs from "@/hooks/api/cameras/useEdgeBoxLogs";
import type { LogRow, LogSeverity } from "@/services/api/cameras";

/**
 * Inline logs panel for the EdgeBoxDetailPage. Moved out of the
 * modal LogViewerDialog so logs become a first-class surface — the
 * operator can scroll, deep-link, and screenshot the page during an
 * incident without losing context.
 *
 * Behaviour mirrors the dialog version (severity + event filter,
 * auto-refresh poll, summary badges), but the host is responsible
 * for sizing — we no longer cap height to viewport.
 */
type Props = {
  workspaceId: string | undefined;
  boxId: string | undefined;
};

const SEVERITIES: { value: LogSeverity | "all"; label: string }[] = [
  { value: "all", label: "All severities" },
  { value: "debug", label: "Debug+" },
  { value: "info", label: "Info+" },
  { value: "warn", label: "Warn+" },
  { value: "error", label: "Error only" }
];

const REFRESH_MS = 5_000;

function severityBadgeVariant(
  severity: string
): "default" | "secondary" | "destructive" | "outline" {
  switch (severity) {
    case "error":
      return "destructive";
    case "warn":
    case "warning":
      return "outline";
    case "debug":
      return "secondary";
    default:
      return "default";
  }
}

function prettyFields(fieldsJson: string): string {
  try {
    const parsed = JSON.parse(fieldsJson);
    if (parsed && typeof parsed === "object" && Object.keys(parsed).length === 0) {
      return "";
    }
    return JSON.stringify(parsed);
  } catch {
    return fieldsJson;
  }
}

const LogsPanel: React.FC<Props> = ({ workspaceId, boxId }) => {
  const [severity, setSeverity] = useState<LogSeverity | "all">("all");
  const [eventQuery, setEventQuery] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(false);

  const {
    data: rows = [],
    isFetching,
    error,
    refetch
  } = useEdgeBoxLogs(
    workspaceId,
    boxId,
    {
      min_severity: severity === "all" ? undefined : severity,
      event_contains: eventQuery.trim() || undefined,
      limit: 500
    },
    { enabled: !!boxId, refetchIntervalMs: autoRefresh ? REFRESH_MS : false }
  );

  const summary = useMemo(() => summarize(rows), [rows]);

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex flex-wrap items-end gap-3'>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='log-severity' className='text-muted-foreground text-xs uppercase'>
            Severity
          </Label>
          <Select value={severity} onValueChange={(v) => setSeverity(v as LogSeverity | "all")}>
            <SelectTrigger id='log-severity' className='h-8 w-40 text-xs'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              {SEVERITIES.map((s) => (
                <SelectItem key={s.value} value={s.value}>
                  {s.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='log-event' className='text-muted-foreground text-xs uppercase'>
            Event contains
          </Label>
          <Input
            id='log-event'
            value={eventQuery}
            onChange={(e) => setEventQuery(e.target.value)}
            placeholder='e.g. vlm_call'
            className='h-8 w-56 text-xs'
          />
        </div>
        <div className='ml-auto flex items-center gap-3'>
          <div className='flex items-center gap-2'>
            <Switch
              id='log-auto-refresh'
              checked={autoRefresh}
              onCheckedChange={setAutoRefresh}
              aria-label='Auto-refresh logs'
            />
            <Label htmlFor='log-auto-refresh' className='text-xs'>
              Auto-refresh
            </Label>
          </div>
          <Button
            variant='outline'
            size='sm'
            onClick={() => refetch()}
            disabled={isFetching}
            aria-label='Refresh logs'
          >
            {isFetching ? (
              <Loader2 className='mr-1.5 h-3.5 w-3.5 animate-spin' />
            ) : (
              <RefreshCw className='mr-1.5 h-3.5 w-3.5' />
            )}
            Refresh
          </Button>
        </div>
      </div>

      <div className='flex items-center gap-2 text-muted-foreground text-xs'>
        <Badge variant='secondary'>{rows.length} rows</Badge>
        {summary.error > 0 && (
          <Badge variant='destructive'>
            {summary.error} {summary.error === 1 ? "error" : "errors"}
          </Badge>
        )}
        {summary.warn > 0 && <Badge variant='outline'>{summary.warn} warn</Badge>}
      </div>

      {error ? (
        <div className='border border-destructive bg-destructive/10 p-3 text-destructive text-sm'>
          Failed to load logs: {error.message}
        </div>
      ) : rows.length === 0 ? (
        <EmptyState fetching={isFetching} />
      ) : (
        <div className='max-h-[70vh] overflow-y-auto border border-border'>
          <table className='w-full font-mono text-xs'>
            <thead className='sticky top-0 z-10 bg-muted text-muted-foreground'>
              <tr>
                <th className='px-2 py-1.5 text-left'>Time</th>
                <th className='px-2 py-1.5 text-left'>Sev</th>
                <th className='px-2 py-1.5 text-left'>Event</th>
                <th className='px-2 py-1.5 text-left'>Fields</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row, i) => (
                <LogRowView key={rowKey(row, i)} row={row} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
};

const LogRowView: React.FC<{ row: LogRow }> = ({ row }) => {
  const fields = prettyFields(row.fields_json);
  return (
    <tr className='border-border border-t align-top'>
      <td className='whitespace-nowrap px-2 py-1.5 text-muted-foreground'>
        {new Date(row.ts).toLocaleTimeString()}
      </td>
      <td className='px-2 py-1.5'>
        <Badge variant={severityBadgeVariant(row.severity)} className='font-mono'>
          {row.severity}
        </Badge>
      </td>
      <td className='whitespace-nowrap px-2 py-1.5'>{row.event}</td>
      <td className='whitespace-pre-wrap break-all px-2 py-1.5 text-muted-foreground'>{fields}</td>
    </tr>
  );
};

const EmptyState: React.FC<{ fetching: boolean }> = ({ fetching }) => (
  <div className='flex flex-col items-center gap-2 border border-border border-dashed p-6 text-muted-foreground text-sm'>
    {fetching ? (
      <>
        <Loader2 className='h-4 w-4 animate-spin' />
        <span>Loading logs…</span>
      </>
    ) : (
      <>
        <span>No log rows match the current filters.</span>
        <span className='text-xs'>
          The shipper batches every 30 seconds — fresh boxes may not have anything to read yet.
        </span>
      </>
    )}
  </div>
);

function rowKey(row: LogRow, i: number): string {
  return `${row.ts}-${row.event}-${i}`;
}

function summarize(rows: LogRow[]): { error: number; warn: number } {
  let error = 0;
  let warn = 0;
  for (const r of rows) {
    if (r.severity === "error") error++;
    else if (r.severity === "warn" || r.severity === "warning") warn++;
  }
  return { error, warn };
}

export default LogsPanel;
