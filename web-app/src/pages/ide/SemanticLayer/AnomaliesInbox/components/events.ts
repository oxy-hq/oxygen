import type {
  AnomalySeverity,
  AnomalyStatus,
  MetricAnomaly,
  StatusWriteGroup
} from "@/types/metricAnomalies";

/** One anomaly event: the buckets it spans, and the worst of them. */
export interface AnomalyEvent {
  /** Stable identity for the event — its `event_id`, or `ungrouped:<row id>`
   *  for rows detected before events existed. Doubles as the React key and the
   *  selection key, so the two can't drift apart. */
  key: string;
  /** The server-side `event_id`, when there is one. Status writes name this
   *  rather than enumerating `buckets`: a list response caps how many buckets
   *  it returns per event, so acking the ids we happen to hold would leave the
   *  tail of a long chain behind while reporting success. `null` for
   *  pre-event rows, which can only be named individually. */
  eventId: string | null;
  /** Every bucket in the event the server sent, oldest first — see
   *  {@link AnomalyEvent.truncated}. */
  buckets: MetricAnomaly[];
  /** The server trimmed this event's buckets to its per-event cap, so
   *  `buckets` is a floor rather than the whole chain. */
  truncated: boolean;
  /** The bucket that departed furthest from expectation — the row's numbers. */
  peak: MetricAnomaly;
  /** The event's status, which is its *least resolved* bucket: `new` if any
   *  bucket is still new, else `acknowledged` if any is, else `dismissed`.
   *
   *  Not the peak's. The peak is chosen by `|z|`, so in the All tab an event
   *  can show an `acknowledged` peak while holding a later `new` bucket — and
   *  the row's actions read every bucket. Labelling it "acknowledged" while
   *  offering Ack, and then changing nothing visible when the hidden bucket
   *  moves, is exactly the "the only sign it landed is a button disappearing"
   *  failure the label exists to prevent. */
  status: AnomalyStatus;
  /**
   * The event's severity: the max over its buckets, not the peak bucket's.
   * A sustained slide files its later days as `low` (they sit inside their
   * seasonal band, kept only because they continue the event), so reading
   * severity off `peak` — chosen by z-score, which can be a later day — would
   * badge a real collapse `low`. Rolling up the max keeps the initial breach's
   * severity on the whole event.
   */
  severity: AnomalySeverity;
}

/** The third copy of one encoding, and the only one outside Rust: the server
 *  ranks pages with `severity_rank_case_sql` and trims events with
 *  `severity_rank`, both in `crates/metric-monitoring/src/detect.rs`. This
 *  ranks buckets inside an event the server already served, so it can't ask for
 *  theirs — a change there is a change here. */
const SEVERITY_RANK: Record<AnomalySeverity, number> = { low: 0, medium: 1, high: 2 };

const rank = (s: AnomalySeverity): number => SEVERITY_RANK[s] ?? 0;

/** The least-resolved status across an event's buckets — what the row is,
 *  as opposed to what its worst bucket happens to be. */
function leastResolved(buckets: MetricAnomaly[]): AnomalyStatus {
  if (buckets.some((b) => b.status === "new")) return "new";
  if (buckets.some((b) => b.status === "acknowledged")) return "acknowledged";
  return "dismissed";
}

/** The most severe severity across an event's buckets. */
function maxSeverity(buckets: MetricAnomaly[]): AnomalySeverity {
  return buckets.reduce<AnomalySeverity>(
    (worst, b) => (rank(b.severity) > rank(worst) ? b.severity : worst),
    "low"
  );
}

/**
 * The statuses a write on this event may touch.
 *
 * `dismissed` is the one state that must never be reversed by accident: the
 * user retired those buckets on purpose, and from the New or Acknowledged tab
 * they cannot even see them. Everything still live — `new` and `acknowledged` —
 * moves together, because a scan chains a fresh `new` bucket onto an event that
 * was already acknowledged, and leaving that one behind would keep the event in
 * the New tab under a toast saying it was handled.
 *
 * Acting from the Dismissed tab is the exception: there the filter guarantees
 * every bucket on screen is dismissed, so those buckets *are* what the user is
 * looking at.
 *
 * Note what that scope does in the other direction, because it is intended
 * rather than overlooked: from the Dismissed tab the live statuses are in scope
 * too, so acking a dismissed row also acks a `new` bucket a later scan chained
 * onto it, and the anomaly leaves the New tab. That is the anomaly moving as
 * one — the same principle that keeps `new` and `acknowledged` together, and
 * the reason a write is not bounded to the single status on screen. The
 * asymmetry is only in which direction is dangerous: sweeping a live bucket
 * along restates a judgement the user is making right now, while sweeping a
 * dismissed one reverses a judgement they made and can no longer see.
 *
 * The decision comes from the view, not from the row's own buckets, and that is
 * load-bearing. A row's `buckets` are what the server *sent*, and a long chain
 * is capped at `MAX_BUCKETS_PER_EVENT` — so an All-tab event of 55 dismissed
 * buckets and 5 mild `new` ones can arrive with the `new` ones trimmed away,
 * look wholly dismissed, widen its own scope, and un-dismiss 55 buckets the
 * user deliberately retired. The client cannot tell a capped event from a
 * complete one; it can always tell which tab it is on.
 *
 * One visible consequence, so the next reader files it as a decision rather
 * than a bug: in **All**, a row whose every bucket is dismissed has nothing in
 * scope, so {@link canMoveTo} is false both ways and it renders with Explain
 * only. That is the honest render — the write would match nothing — and the
 * Dismissed tab is where such a row is actionable.
 */
function writeScope(viewing: AnomalyStatus | "all"): AnomalyStatus[] {
  return viewing === "dismissed" ? [...LIVE_STATUSES, "dismissed"] : [...LIVE_STATUSES];
}

/**
 * Would moving this event to `status` change anything the write is allowed to
 * touch?
 *
 * Both halves matter, and asking only one of them is how the button and the
 * write come apart.
 *
 * Every bucket, not the peak: the **All** tab returns an event's buckets in
 * whatever state they are in and picks the peak by `|z|`, so an event holding
 * one `new` bucket under an `acknowledged` peak would hide its own Ack button
 * while the New badge kept counting it.
 *
 * And only buckets inside {@link writeScope}: outside the Dismissed tab a row
 * can show an event whose only differing bucket is `dismissed`, and therefore
 * out of scope. Ack would render, the write would match nothing, and the toast
 * would report the rows as gone while they sat on screen — with the button
 * still there to click again.
 *
 * Shared by the row buttons and the batch bar's counts so the two can't
 * disagree about whether an action does anything.
 */
export function canMoveTo(
  event: AnomalyEvent,
  status: AnomalyStatus,
  viewing: AnomalyStatus | "all"
): boolean {
  const scope = writeScope(viewing);
  return event.buckets.some((b) => scope.includes(b.status) && b.status !== status);
}

/**
 * What to send a status write for these events.
 *
 * Two things are load-bearing here.
 *
 * An event with an `event_id` is named by it, so the server resolves every
 * bucket — including any the list response capped away. Only pre-event rows,
 * which have no event to name, fall back to their own ids.
 *
 * And an event can span statuses, so the write carries the statuses it may
 * touch — see {@link writeScope}. One group, not one per row: the scope depends
 * on the view, so every row in a selection shares it and the whole selection is
 * a single write.
 */
export function targetOf(
  events: AnomalyEvent[],
  viewing: AnomalyStatus | "all"
): StatusWriteGroup | null {
  if (events.length === 0) return null;
  const onlyStatuses = writeScope(viewing);
  const group: StatusWriteGroup = { onlyStatuses, ids: [], eventIds: [] };
  for (const event of events) {
    if (event.eventId) group.eventIds.push(event.eventId);
    else group.ids.push(...event.buckets.map((b) => b.id));
  }
  return group;
}

/** Statuses an anomaly can still be acted on from — everything but the one the
 *  user has already retired it into. */
const LIVE_STATUSES: AnomalyStatus[] = ["new", "acknowledged"];

/**
 * Collapse per-bucket rows into events.
 *
 * A sustained problem files one row per bucket, so a three-day labour surge
 * reads as three separate anomalies and "how many problems do I have" cannot be
 * answered by counting rows. Rows stay per-bucket on the server because
 * `explain` reasons about a single bucket, so the collapsing lives here.
 *
 * Rows with no `event_id` (detected before events existed) each stand alone
 * rather than being lumped together under a shared null key.
 *
 * Lives apart from the table component so the grouping — which selection keys
 * and the bulk action both depend on — can be tested without pulling React and
 * the API layer into the test.
 */
export function groupIntoEvents(
  anomalies: MetricAnomaly[],
  truncatedEvents: string[] = []
): AnomalyEvent[] {
  const truncated = new Set(truncatedEvents);
  const byEvent = new Map<string, MetricAnomaly[]>();
  for (const a of anomalies) {
    const key = a.event_id ?? `ungrouped:${a.id}`;
    const existing = byEvent.get(key);
    if (existing) existing.push(a);
    else byEvent.set(key, [a]);
  }
  return Array.from(byEvent.entries()).map(([key, buckets]) => {
    const ordered = [...buckets].sort((x, y) => x.period_start.localeCompare(y.period_start));
    // The peak, not the latest: the worst day is what an operator triages on,
    // and it is the bucket whose explain is worth reading.
    const peak = ordered.reduce((a, b) => (Math.abs(b.z_score) > Math.abs(a.z_score) ? b : a));
    return {
      key,
      eventId: ordered[0].event_id,
      buckets: ordered,
      truncated: truncated.has(key),
      peak,
      status: leastResolved(ordered),
      severity: maxSeverity(ordered)
    };
  });
}
