/**
 * Timeline of raw automation events. Available to all users (was
 * admin-only when first introduced).
 *
 * Renders the SSE event stream the run page already consumes, but
 * surfaces the coordinator/worker/decider trace events
 * (`worker_task_claimed`, `task_failed`, `waiting_on_children`,
 * `decider_decided`) that the user-facing Output view collapses
 * into per-step rows.
 *
 * Each row carries:
 * - A sequence index (the SSE arrival order, useful for cross-referencing
 *   tracing logs).
 * - The event type in mono.
 * - A short summary (same shape `summarize` produces).
 * - An expand toggle that reveals the raw JSON payload — debugging
 *   a stuck run usually wants the full picture, not just the summary.
 *
 * Failure-ish events (`task_failed`, `decider_decided { kind: "fail" }`,
 * `subrun_step_completed { success: false }`, `subrun_completed
 * { success: false }`) render in the destructive color so they're easy
 * to spot in a long stream.
 *
 * Note: `delegation_retry` and `delegation_fallback` are intentionally
 * NOT routed to the FE — `task_failed` carries the same attempt-level
 * detail (attempt number + spec_kind + step name + error) and
 * subsumes them. See `event_registry.rs` for the SSE allowlist.
 */

import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import type { AutomationEvent } from "@/services/api/automations";

type Props = {
  events: AutomationEvent[];
};

export const Trace = ({ events }: Props) => {
  if (events.length === 0) {
    return (
      <div className='p-4 text-muted-foreground text-sm'>
        No events yet — the worker hasn't claimed this run.
      </div>
    );
  }
  return (
    <ul className='flex flex-col gap-0.5 p-2 font-mono text-xs'>
      {events.map((event, idx) => (
        <TraceRow key={idx} index={idx} event={event} />
      ))}
    </ul>
  );
};

const TraceRow = ({ index, event }: { index: number; event: AutomationEvent }) => {
  const [expanded, setExpanded] = useState(false);
  const isFailure = isFailureEvent(event);
  const Icon = expanded ? ChevronDown : ChevronRight;

  return (
    <li>
      <button
        type='button'
        onClick={() => setExpanded((v) => !v)}
        className='flex w-full items-baseline gap-2 rounded px-1.5 py-0.5 text-left hover:bg-muted'
      >
        <Icon className='size-3 shrink-0 self-center text-muted-foreground' />
        <span className='w-12 shrink-0 text-muted-foreground tabular-nums'>
          #{String(index).padStart(3, "0")}
        </span>
        <span className={`shrink-0 ${isFailure ? "text-destructive" : "text-foreground"}`}>
          {event.type}
        </span>
        <span className='truncate text-muted-foreground'>{summarize(event)}</span>
      </button>
      {expanded && (
        <pre className='mt-1 mb-2 ml-12 overflow-x-auto whitespace-pre-wrap break-all rounded bg-muted px-2 py-1 text-[11px]'>
          {JSON.stringify(event.payload, null, 2)}
        </pre>
      )}
    </li>
  );
};

function summarize(event: AutomationEvent): string {
  switch (event.type) {
    case "subrun_started":
      return `${event.payload.subrun_name} (${event.payload.steps.length} steps)`;
    case "subrun_step_started":
      return event.payload.step;
    case "subrun_step_cache_hit": {
      const src = event.payload.source === "file" ? event.payload.path : event.payload.prior_run_id;
      return `${event.payload.step} ← ${src ?? "cache"}`;
    }
    case "subrun_step_output":
      return `${event.payload.step} → output`;
    case "subrun_step_completed":
      return `${event.payload.step} ${event.payload.cached ? "(cached)" : event.payload.success ? "✓" : "✗"}${
        event.payload.error ? ` — ${event.payload.error}` : ""
      }`;
    case "subrun_completed":
      return `${event.payload.subrun_name} ${event.payload.success ? "✓" : "✗"}`;
    case "subrun_step_iteration_started":
      return `${event.payload.step} [${event.payload.index}] started`;
    case "subrun_step_iteration_completed": {
      const p = event.payload;
      const mark = p.status === "done" ? "✓" : p.status === "failed" ? "✗" : "⊘";
      const err = p.error ? ` — ${p.error}` : "";
      return `${p.step} [${p.index}] ${mark}${err}`;
    }
    case "task_failed": {
      const kind = event.payload.spec_kind ?? "task";
      const step = event.payload.step_name ? ` step=${event.payload.step_name}` : "";
      return `${kind}@attempt=${event.payload.attempt}${step} ✗ ${event.payload.error}`;
    }
    case "worker_task_claimed":
      return `${event.payload.spec_kind} → claimed`;
    case "waiting_on_children":
      return `waiting on ${event.payload.child_task_ids.length} child(ren)`;
    case "decider_decided": {
      const p = event.payload;
      switch (p.kind) {
        case "delegate_step":
          return `→ delegate_step ${p.step_name ?? "?"} (${p.target?.kind ?? "?"})`;
        case "delegate_parallel":
          return `→ delegate_parallel ${p.step_name ?? "?"} ×${p.item_count ?? 0}`;
        case "step_executed_inline":
          return `→ inline ${p.step_name ?? "?"}`;
        case "wait_for_more_children":
          return "→ wait_for_more_children";
        case "complete":
          return "→ complete";
        case "fail":
          return `→ fail · ${p.error ?? ""}`;
        default:
          // The union is exhausted above, so `p` is `never` here — but the payload
          // comes off the wire, and a newer backend can send a kind this build
          // does not know. Read it as the string it is.
          return `→ ${(p as { kind?: string }).kind ?? "?"}`;
      }
    }
    default:
      return (event as { type?: string }).type ?? "";
  }
}

function isFailureEvent(event: AutomationEvent): boolean {
  switch (event.type) {
    case "task_failed":
      return true;
    case "subrun_step_completed":
    case "subrun_completed":
      return !event.payload.success;
    case "decider_decided":
      return event.payload.kind === "fail";
    default:
      return false;
  }
}
