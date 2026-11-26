import { cn } from "@/libs/shadcn/utils";
import type { TimelineSpan } from "@/services/api/traces";
import { formatDuration } from "../../utils/index";
import { KeyValueGrid, type KeyValueRow } from "./InspectorKeyValue";
import { getSpanCategory, isErrorStatus, SPAN_CATEGORY_META } from "./spanCategory";
import { getSpanError, getSpanModel, getSpanRows, getSpanTokens } from "./spanInspect";

interface InspectorOverviewProps {
  span: TimelineSpan;
  selfMs: number;
}

export function InspectorOverview({ span, selfMs }: InspectorOverviewProps) {
  const category = getSpanCategory(span);
  const { barClass } = SPAN_CATEGORY_META[category];
  const childrenMs = Math.max(0, span.durationMs - selfMs);
  const selfPct = span.durationMs > 0 ? (selfMs / span.durationMs) * 100 : 100;
  const childPct = 100 - selfPct;

  const model = getSpanModel(span);
  const tokens = getSpanTokens(span);
  const rows = getSpanRows(span);
  const error = getSpanError(span);
  const isError = isErrorStatus(span.statusCode);

  const kv: KeyValueRow[] = [];
  if (model) kv.push({ label: "model", value: model });
  if (tokens) {
    kv.push({
      label: "tokens",
      value:
        tokens.input !== undefined || tokens.output !== undefined
          ? `${(tokens.input ?? 0).toLocaleString()} in · ${(tokens.output ?? 0).toLocaleString()} out`
          : tokens.total.toLocaleString()
    });
  }
  if (rows !== undefined) kv.push({ label: "rows returned", value: rows.toLocaleString() });
  if (error) kv.push({ label: "error", value: error, tone: "error" });
  kv.push({ label: "category", value: category });
  if (span.spanKind) kv.push({ label: "span kind", value: span.spanKind });
  kv.push({
    label: "status",
    value: span.statusCode || "Unset",
    tone: isError ? "error" : "default"
  });

  return (
    <div className='space-y-4'>
      {/* Self vs children timing bar */}
      <div className='space-y-2'>
        <div className='flex items-center justify-between text-muted-foreground text-xs'>
          <span className='uppercase tracking-wide'>timing</span>
          <span className='font-mono tabular-nums'>{formatDuration(span.durationMs)} total</span>
        </div>
        <div className='flex h-2 overflow-hidden bg-muted'>
          <div className={cn("h-full", barClass)} style={{ width: `${selfPct}%` }} />
          <div className='h-full bg-muted-foreground/30' style={{ width: `${childPct}%` }} />
        </div>
        <KeyValueGrid
          rows={[
            { label: "self time", value: formatDuration(selfMs) },
            { label: "in children", value: formatDuration(childrenMs) },
            { label: "start offset", value: `+${formatDuration(span.offsetMs)}` }
          ]}
        />
      </div>

      <div className='border-t pt-3'>
        <KeyValueGrid rows={kv} />
      </div>
    </div>
  );
}
