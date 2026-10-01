/**
 * Formatting helpers for the Compliance tab. Pulled into their own
 * module so the list page and the detail page can share semantics
 * (verdict color, snippet truncation, segment-range rendering)
 * without one importing the other.
 */

/**
 * The VLM's structured verdict. The edge worker prompt asks for
 * exactly these fields (see `video-poc/edge/worker/camera.py` —
 * `VLM_PROMPT` constant). We keep the shape permissive because the
 * VLM occasionally returns extra keys or misses one.
 */
export type StructuredVerdict = {
  attire_compliant?: boolean;
  hygiene_compliant?: boolean;
  missing_items?: string[];
  confidence?: number;
  notes?: string;
};

/**
 * Coerce the server's `unknown` (the row's `structured_json` was
 * deserialized by serde but we don't have a Zod schema yet) into the
 * expected shape. Returns an empty object rather than throwing —
 * a malformed VLM output should still render a usable row.
 */
export function toStructuredVerdict(raw: unknown): StructuredVerdict {
  if (!raw || typeof raw !== "object") return {};
  const v = raw as Record<string, unknown>;
  return {
    attire_compliant: typeof v.attire_compliant === "boolean" ? v.attire_compliant : undefined,
    hygiene_compliant: typeof v.hygiene_compliant === "boolean" ? v.hygiene_compliant : undefined,
    missing_items: Array.isArray(v.missing_items)
      ? v.missing_items.filter((s): s is string => typeof s === "string")
      : undefined,
    confidence: typeof v.confidence === "number" ? v.confidence : undefined,
    notes: typeof v.notes === "string" ? v.notes : undefined
  };
}

/**
 * A report counts as a violation when EITHER attire or hygiene is
 * `false`. Missing fields default to "not a violation" so a parse
 * failure doesn't read as a false alarm.
 */
export function isComplianceFailure(v: StructuredVerdict): boolean {
  return v.attire_compliant === false || v.hygiene_compliant === false;
}

/**
 * Format the wall-clock time the report was received by Oxy. Used as
 * the primary timestamp in the list — the segment range (when the
 * worker was actually looking at the person) goes underneath as
 * secondary text.
 */
export function formatReceivedAt(iso: string): string {
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit"
  });
}

/**
 * Duration in seconds, used as the `?duration=` query for the clip
 * playback proxy. Clamped at 600 to match the server-side cap.
 */
export function segmentDurationSeconds(startIso: string, endIso: string): number {
  const start = new Date(startIso).getTime();
  const end = new Date(endIso).getTime();
  const seconds = Math.max(1, Math.round((end - start) / 1000));
  return Math.min(seconds, 600);
}

/**
 * Truncate a VLM report for the list view. The full text shows up on
 * the detail page; here we just want enough to skim.
 */
export function reportSnippet(text: string, max = 140): string {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  return `${flat.slice(0, max - 1)}…`;
}

/**
 * Shared date-range presets — used by both the site rollup view and
 * the per-camera report list. Keeping them in one place means the
 * two views always offer the same windows, so an operator's mental
 * model of "last 24 hours" doesn't shift between drill-down levels.
 */
export const RANGE_OPTIONS: { value: string; label: string; hours: number | null }[] = [
  { value: "24h", label: "Last 24 hours", hours: 24 },
  { value: "7d", label: "Last 7 days", hours: 24 * 7 },
  { value: "30d", label: "Last 30 days", hours: 24 * 30 },
  { value: "all", label: "All time", hours: null }
];

/**
 * Resolve a range key to an ISO-8601 `since` cutoff. `undefined`
 * means "no filter" (matches the server contract: omit `since` to
 * get all rows).
 */
export function rangeToSince(value: string): string | undefined {
  const opt = RANGE_OPTIONS.find((o) => o.value === value);
  if (!opt || opt.hours == null) return undefined;
  return new Date(Date.now() - opt.hours * 60 * 60 * 1000).toISOString();
}

/**
 * Lift the actual server error body out of an axios failure.
 *
 * The compliance routes return JSON with `{ code, message }` on 4xx
 * and 5xx (see `crates/cameras/src/routes/errors.rs`). Without this
 * helper the UI shows axios's generic "Request failed with status
 * code 502", which is useless for debugging — the real message
 * (e.g. "airhouse error: connect failed: …") sits on `.response.data`.
 */
export function apiErrorMessage(err: unknown): string {
  if (err && typeof err === "object") {
    const e = err as {
      response?: { data?: { message?: unknown; code?: unknown } };
      message?: unknown;
    };
    const body = e.response?.data;
    if (body && typeof body.message === "string" && body.message.length > 0) {
      return body.message;
    }
    if (body && typeof body.code === "string" && body.code.length > 0) {
      return body.code;
    }
    if (typeof e.message === "string" && e.message.length > 0) {
      return e.message;
    }
  }
  return "Unknown error";
}

/**
 * Three-tier severity ladder for the timeline.
 *
 * - `critical` — violation that landed within the last hour. These
 *   are the events an operator should act on right now.
 * - `warning` — older violation. Still a violation; still surfaces
 *   in counts, but visually de-emphasized.
 * - `info` — compliant verdict. Useful as ambient signal ("the
 *   worker is awake and looking") but not actionable.
 *
 * The 1h cutoff matches the worker's dwell-trigger cadence — a single
 * shift is usually 4–8h so "this hour" is a tight enough window to
 * read as "happening now" rather than "happened today."
 */
export type Severity = "critical" | "warning" | "info";

const CRITICAL_WINDOW_MS = 60 * 60 * 1000;

export function eventSeverity(
  structuredJson: unknown,
  receivedAtIso: string,
  now: number = Date.now()
): Severity {
  const failure = isComplianceFailure(toStructuredVerdict(structuredJson));
  if (!failure) return "info";
  const ts = new Date(receivedAtIso).getTime();
  return now - ts <= CRITICAL_WINDOW_MS ? "critical" : "warning";
}

/** Tailwind class for the colored dot / tick / accent in the timeline. */
export function severityDotClass(s: Severity): string {
  switch (s) {
    case "critical":
      return "bg-destructive";
    case "warning":
      return "bg-amber-500";
    case "info":
      return "bg-emerald-500";
  }
}

export function severityLabel(s: Severity): string {
  switch (s) {
    case "critical":
      return "critical";
    case "warning":
      return "warning";
    case "info":
      return "info";
  }
}
