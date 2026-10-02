import {
  CheckCircle2,
  CircleMinus,
  CircleSlash,
  Loader2,
  ShieldCheck,
  XCircle
} from "lucide-react";
import type React from "react";
import { useState } from "react";
import type { RunEventEntry } from "@/services/api/coordinator";

/** Collapsible summary of a failed rollup's error message. */
const FailedRollupRow: React.FC<{ label: string; error?: string }> = ({ label, error }) => {
  const [expanded, setExpanded] = useState(false);
  const summary = error
    ? (error.includes(" SQL:") ? error.slice(0, error.indexOf(" SQL:")) : error).slice(0, 160)
    : undefined;
  const hasMore = error && error.length > (summary?.length ?? 0);

  return (
    <div className='space-y-0.5'>
      <div className='flex items-center gap-1.5 text-destructive text-xs'>
        <XCircle className='h-3 w-3 shrink-0' />
        <span>{label}</span>
      </div>
      {summary && (
        <div className='space-y-0.5 pl-5'>
          <p className='text-muted-foreground text-xs leading-relaxed'>
            {expanded ? error : summary}
            {hasMore && !expanded && <span>…</span>}
          </p>
          {hasMore && (
            <button
              type='button'
              onClick={() => setExpanded((v) => !v)}
              className='text-muted-foreground text-xs underline underline-offset-2 hover:text-foreground'
            >
              {expanded ? "show less" : "show more"}
            </button>
          )}
        </div>
      )}
    </div>
  );
};

/**
 * `view` and `rollup` are strings on every pre-aggregation event that carries them
 * (the backend's `PreaggEvent`); the payload type is an open record, so anything else
 * reads as absent.
 */
const payloadText = (value: unknown): string => (typeof value === "string" ? value : "");

/**
 * Streamed event log for a run node — pre-aggregation rollup progress. Drives
 * the ELT debugging unit (freshness + rollup health).
 */
export const EventLog: React.FC<{ events: RunEventEntry[] }> = ({ events }) => {
  if (events.length === 0) return null;

  // The three ways a started rebuild ends. A retraction is one of them — the rebuild ran
  // and removed an empty rollup — so its `started` row must stop spinning too.
  const settled = new Set(
    events
      .filter(
        (e) =>
          e.event_type === "preagg_rollup_done" ||
          e.event_type === "preagg_rollup_retracted" ||
          e.event_type === "preagg_rollup_failed"
      )
      .map((e) => `${payloadText(e.payload.view)}.${payloadText(e.payload.rollup)}`)
  );

  return (
    <div className='space-y-0.5'>
      {events.map((ev) => {
        const view = payloadText(ev.payload.view);
        const rollup = payloadText(ev.payload.rollup);
        const error = ev.payload.error as string | undefined;
        const label = view && rollup ? `${view}.${rollup}` : ev.event_type;

        if (ev.event_type === "preagg_rollup_fresh") {
          return (
            <div key={ev.seq} className='flex items-center gap-1.5 text-muted-foreground text-xs'>
              <ShieldCheck className='h-3 w-3 shrink-0' />
              <span>{label}</span>
              <span className='opacity-60'>— up to date</span>
            </div>
          );
        }
        if (ev.event_type === "preagg_rollup_started") {
          if (settled.has(label)) return null;
          return (
            <div key={ev.seq} className='flex items-center gap-1.5 text-muted-foreground text-xs'>
              <Loader2 className='h-3 w-3 shrink-0 animate-spin' />
              <span>{label}</span>
            </div>
          );
        }
        if (ev.event_type === "preagg_rollup_done") {
          return (
            <div key={ev.seq} className='flex items-center gap-1.5 text-success text-xs'>
              <CheckCircle2 className='h-3 w-3 shrink-0' />
              <span>{label}</span>
            </div>
          );
        }
        if (ev.event_type === "preagg_rollup_retracted") {
          // Not a failure: the rebuild ran, found no rows, and removed the rollup rather
          // than leave it serving the previous build.
          return (
            <div key={ev.seq} className='flex items-center gap-1.5 text-muted-foreground text-xs'>
              <CircleMinus className='h-3 w-3 shrink-0' />
              <span>{label}</span>
              <span className='opacity-60'>— no rows, rollup removed</span>
            </div>
          );
        }
        if (ev.event_type === "preagg_rollup_skipped_no_refresh_key") {
          return null;
        }
        if (ev.event_type === "preagg_rollup_skipped_no_datasource") {
          const database = payloadText(ev.payload.database);
          return (
            <div key={ev.seq} className='flex items-center gap-1.5 text-muted-foreground text-xs'>
              <CircleSlash className='h-3 w-3 shrink-0' />
              <span>{label}</span>
              <span className='opacity-60'>
                — skipped, datasource {database ? `"${database}" ` : ""}not configured
              </span>
            </div>
          );
        }
        if (ev.event_type === "preagg_rollup_failed") {
          return <FailedRollupRow key={ev.seq} label={label} error={error} />;
        }
        if (ev.event_type === "preagg_refresh_key_error") {
          // Carries a rollup hash rather than a view and rollup name.
          const hash = payloadText(ev.payload.rollup_hash);
          return (
            <FailedRollupRow
              key={ev.seq}
              label={hash ? `refresh key check failed (${hash})` : "refresh key check failed"}
              error={error}
            />
          );
        }
        // An event type this component does not know yet. Shown neutrally: defaulting to
        // the failed row is how a retraction came to render as a failure.
        return (
          <div key={ev.seq} className='flex items-center gap-1.5 text-muted-foreground text-xs'>
            <span>{label}</span>
            {label !== ev.event_type && <span className='opacity-60'>— {ev.event_type}</span>}
          </div>
        );
      })}
    </div>
  );
};
