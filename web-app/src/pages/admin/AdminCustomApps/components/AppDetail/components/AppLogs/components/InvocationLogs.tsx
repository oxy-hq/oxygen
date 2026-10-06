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
 * The widths of the time and level columns.
 *
 * Fixed, and shared by the list's heading and every line, because the lines
 * are separate flex rows: a width taken from a row's own content is a
 * different width on every row, and the messages would start at a ragged edge.
 * A clock time is always eight characters and a dated one fourteen, so the two
 * widths are all there is.
 */
export const LOG_TRACKS = {
  time: (withDate: boolean) => (withDate ? "w-26" : "w-15"),
  level: "w-10"
};

/**
 * One invocation: what ran, the ids that find it elsewhere, then what it
 * printed.
 *
 * The ids are the point of the header. `request` is the `x-oxy-request-id` a
 * support ticket quotes and the serve row carries; `trace` is what finds the
 * invocation's spans in HyperDX. Each has its own labelled slot, so the one
 * being looked for is found by its name rather than read out of a sentence.
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
    <div
      className='border-border/60 border-b last:border-b-0'
      data-testid='admin-app-logs-invocation'
    >
      {/* Two groups, pushed apart. On one line the ids sit at the right; when
          the dossier is too narrow for that they wrap as a group and start at
          the left under the name, instead of floating right on a line of
          their own. */}
      <div className='flex flex-wrap items-center justify-between gap-x-3 gap-y-0.5 bg-muted/40 px-3 py-1.5 text-xs'>
        <span className='flex min-w-0 items-center gap-2'>
          <span className={cn("truncate font-medium", hasError && ADMIN_TONE.danger.text)}>
            {head.function_name}
          </span>
          <span className='shrink-0 rounded-full border border-border px-1.5 text-[10px] text-muted-foreground leading-4'>
            {head.mode}
          </span>
        </span>
        <span className='flex flex-wrap items-center gap-x-3'>
          {head.request_id && (
            <LabelledId
              label='Request'
              value={head.request_id}
              testId='admin-app-logs-request-id'
            />
          )}
          {head.trace_id && (
            <LabelledId label='Trace' value={head.trace_id} testId='admin-app-logs-trace-id' />
          )}
        </span>
      </div>
      <div className='py-1'>
        {lines.map((line) => (
          <div key={line.seq} className='flex items-baseline gap-3 px-3 py-0.5 text-xs'>
            <span
              className={cn(
                "shrink-0 font-mono text-muted-foreground tabular-nums",
                LOG_TRACKS.time(withDate)
              )}
              title={line.timestamp}
            >
              {formatLogTime(line.timestamp, withDate)}
            </span>
            {/* The level as a word. Colour alone said it before, which a
                reader who cannot tell the two greys apart never saw. */}
            <span
              className={cn(
                "shrink-0 text-[10px] uppercase tracking-[0.16em]",
                LOG_TRACKS.level,
                levelClass(line.level)
              )}
            >
              {line.level === "warn" || line.level === "error" ? line.level : ""}
            </span>
            <span
              className={cn(
                "min-w-0 whitespace-pre-wrap break-words font-mono",
                levelClass(line.level)
              )}
            >
              {line.message}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
};

const LabelledId = ({ label, value, testId }: { label: string; value: string; testId: string }) => (
  <span className='flex items-center gap-0.5' data-testid={testId}>
    <span className='text-muted-foreground'>{label}</span>
    <CopyableId value={value} />
  </span>
);
