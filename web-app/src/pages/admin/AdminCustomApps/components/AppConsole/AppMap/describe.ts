import {
  type MapEdge,
  type MapEntry,
  type MapFreshness,
  type MapFunction,
  type MapTarget,
  TARGET_GROUP_LABEL
} from "./appMapModel";

/** The words the diagram's tooltips, the list and the freshness line share, so the
 *  picture and its text fallback cannot say two different things. */

const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;

/** A function's recorded runs: `short` for the diagram node, long for the list. Short
 *  says nothing until there is a run to count — fifty rows of "no runs" crowd out the
 *  names, and the freshness line already names the functions with none. */
export function runsLabel(fn: MapFunction, short = false): string {
  if (fn.runs === null) return short ? "" : "runs not read yet";
  if (fn.runs === 0) return short ? "" : "no recorded run";
  if (short) return plural(fn.runs, "run");
  const failed = fn.failedRuns > 0 ? `, ${fn.failedRuns} failed` : "";
  return `${plural(fn.runs, "recorded run")}${failed}`;
}

export function entryText(entry: MapEntry): string {
  return entry.detail ? `${entry.label} ${entry.detail}` : entry.label;
}

/** How a function reached a target, for the list: families, then counts. */
export function touchText(edge: MapEdge): string {
  const how = edge.families.length > 0 ? edge.families.join(", ") : "declared destination";
  if (!edge.observed) return `${how}, not written to in recent runs`;
  const failed = edge.failedCalls > 0 ? `, ${edge.failedCalls} failed` : "";
  return `${how}, ${plural(edge.calls, "call")}${failed}`;
}

/** A target as one line: what it is, what was seen of it, and how it went. */
export function targetSummary(target: MapTarget): string {
  const details = target.details.length > 0 ? ` (${target.details.join(", ")})` : "";
  const head = `${TARGET_GROUP_LABEL[target.group]}: ${target.label}${details}`;
  if (!target.observed) return `${head}. Declared in the manifest; no recorded run wrote to it.`;
  const failed = target.failedCalls > 0 ? `, ${target.failedCalls} failed` : "";
  return `${head}. ${plural(target.calls, "call")} in recent runs${failed}.`;
}

const QUIET_NAMED = 3;

/**
 * How fresh the right-hand side is, as sentences: the bound it was read under, how
 * many runs that came to and the span they cover, and the functions it has nothing
 * on. `ago` renders a timestamp ("2 hours ago"); injected so this stays pure.
 */
export function freshnessText(f: MapFreshness, ago: (iso: string) => string): string[] {
  const sentences: string[] = [];
  if (f.runs === 0 || !f.oldest || !f.newest) {
    sentences.push(
      `No recorded run in the last ${plural(f.retentionDays, "day")}, so only what the manifest declares is drawn on the right.`
    );
  } else {
    const span =
      f.oldest === f.newest ? ago(f.newest) : `from ${ago(f.oldest)} to ${ago(f.newest)}`;
    sentences.push(
      `Touches come from each function's last ${f.perFunction} recorded runs: ${plural(f.runs, "run")}, ${span}. Traces are kept ${plural(f.retentionDays, "day")}.`
    );
  }
  if (f.quiet.length > 0 && f.runs > 0) {
    const named = f.quiet.slice(0, QUIET_NAMED).join(", ");
    const more = f.quiet.length > QUIET_NAMED ? ` and ${f.quiet.length - QUIET_NAMED} more` : "";
    sentences.push(`No recorded run for ${named}${more}.`);
  }
  return sentences;
}
