import type { UnvaluedReason } from "@/types/metricTree";
import { ConfidenceMark } from "./ConfidenceMark";
import { MeasureChange } from "./MeasureChange";
import { formatValue, type ScenarioNodeData } from "./nodeValue";

const UNVALUED_COPY: Record<UnvaluedReason, string> = {
  no_rows_in_window: "no rows in window",
  query_failed: "couldn't load a value",
  // Rows came back and carried none of these measures — the opposite fix from
  // "no rows in window", so it must not share that copy.
  no_matching_columns: "not in this time dimension",
  // Nothing was asked about this node's view. Not a statement about the
  // window, so it must not borrow the copy of the three that ran.
  not_queried: "not read in this window"
};

/**
 * The one line of numbers under a scenario node's label. One branch per node
 * state, because each state has a different thing to say and only one of them
 * is "baseline → simulated".
 */
export function ScenarioValueRow({ data }: { data: ScenarioNodeData }) {
  const { state, baseline, simulated, delta, confidence, leverRaw, unvaluedReason } = data;

  if (state === "unreachable") return null;

  if (state === "unvalued") {
    return (
      <p className='text-[10px] text-muted-foreground'>
        {/* `?? "no baseline value"` as well as the presence check: the same
            unguarded index over a wire-narrowed union that `ConfidenceMark`
            just fixed. A reason the server grows later would otherwise render
            an empty line — softer than throwing, but still a row that says
            nothing at all. */}
        {(unvaluedReason && UNVALUED_COPY[unvaluedReason]) || "no baseline value"}
      </p>
    );
  }

  if (state === "lever") {
    return (
      <div className='flex flex-wrap items-baseline gap-1.5'>
        <span className='bg-primary/15 px-1 py-0.5 font-medium text-[9px] text-primary'>LEVER</span>
        {baseline === undefined && delta === undefined ? (
          // Neither valued nor resolvable — an absolute target typed against a
          // measure with no baseline. What was typed is all there is to show.
          <span className='font-mono text-foreground text-xs tabular-nums'>{leverRaw}</span>
        ) : (
          <MeasureChange
            baseline={baseline}
            simulated={simulated}
            delta={delta}
            format={formatValue}
          />
        )}
      </div>
    );
  }

  if (state === "unchanged") {
    return (
      <div className='flex items-baseline gap-1.5'>
        <span className='font-mono text-muted-foreground text-xs tabular-nums'>
          {baseline !== undefined ? formatValue(baseline) : "—"}
        </span>
        <span className='text-[10px] text-muted-foreground'>unchanged</span>
      </div>
    );
  }

  if (state === "unquantifiable") {
    // `estimated_delta` is 0.0 for an unquantifiable impact, meaning UNKNOWN,
    // not zero. Rendering that 0 would be the surface telling a lie, so this
    // branch never touches `simulated` or `delta`.
    return (
      <div className='flex flex-wrap items-baseline gap-1.5'>
        <span className='font-mono text-xs tabular-nums'>
          {baseline !== undefined ? formatValue(baseline) : "—"}
        </span>
        <span className='text-muted-foreground'>→</span>
        <ConfidenceMark confidence='unquantifiable' withLabel />
      </div>
    );
  }

  return (
    <MeasureChange
      baseline={baseline}
      simulated={simulated}
      delta={delta}
      format={formatValue}
      confidence={confidence}
    />
  );
}
