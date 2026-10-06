import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import { formatLogTime, type InvocationLogs as InvocationLogsGroup } from "../functionLogs";

/** Level → the muted/foreground/danger ladder. */
const levelClass = (level: string) => {
  if (level === "error") return ADMIN_TONE.danger.text;
  if (level === "warn") return "text-foreground";
  return "text-muted-foreground";
};

/**
 * One invocation: what ran, the ids that find it elsewhere, then what it
 * printed.
 *
 * The ids are the point of the header. `request` is the `x-oxy-request-id` a
 * support ticket quotes and the serve row carries; `trace` is what finds the
 * invocation's spans in HyperDX. Both were already in the response and shown
 * nowhere.
 */
export const InvocationLogs = ({
  group,
  withDate
}: {
  group: InvocationLogsGroup;
  withDate: boolean;
}) => {
  const { head, lines, hasError } = group;
  return (
    <div className='border-b last:border-b-0' data-testid='admin-app-logs-invocation'>
      <div className='flex flex-wrap items-center gap-x-2 bg-muted/40 px-3 py-0.5 text-xs'>
        <span className={cn("font-medium", hasError && ADMIN_TONE.danger.text)}>
          {head.function_name}
        </span>
        <span className='text-muted-foreground'>{head.mode}</span>
        <span className='ml-auto flex items-center gap-2 text-muted-foreground'>
          {head.request_id && (
            <span className='flex items-center' data-testid='admin-app-logs-request-id'>
              request
              <CopyableId value={head.request_id} />
            </span>
          )}
          {head.trace_id && (
            <span className='flex items-center' data-testid='admin-app-logs-trace-id'>
              trace
              <CopyableId value={head.trace_id} />
            </span>
          )}
        </span>
      </div>
      {lines.map((line) => (
        <div key={line.seq} className='flex gap-2 px-3 py-1 font-mono text-xs'>
          <span className='shrink-0 text-muted-foreground tabular-nums' title={line.timestamp}>
            {formatLogTime(line.timestamp, withDate)}
          </span>
          <span className={cn("min-w-0 whitespace-pre-wrap break-words", levelClass(line.level))}>
            {line.message}
          </span>
        </div>
      ))}
    </div>
  );
};
