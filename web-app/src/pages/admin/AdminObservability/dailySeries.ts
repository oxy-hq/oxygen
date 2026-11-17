import type { CostDay } from "@/services/api/adminObservability";

/** One calendar day of the window, with the same day one window earlier. */
export interface AlignedDay {
  day: string;
  cost_usd: number;
  run_count: number;
  /** `null` when the prior window was not read. */
  prior_cost_usd: number | null;
}

const DAY_MS = 86_400_000;

function isoDay(t: number): string {
  return new Date(t).toISOString().slice(0, 10);
}

/**
 * Every calendar day of the window, oldest first, each paired with the day
 * `days` earlier.
 *
 * The API returns only days that had runs, so a quiet day would otherwise
 * vanish from the chart and the bars would close up over it — a three-day
 * outage drawn as nothing at all. Filling the calendar makes a zero a zero,
 * and gives the prior window a day to line up against.
 *
 * Days are UTC, as `date_trunc('day', …)` on the server produces them. The
 * window is `days + 1` calendar days because "the last 30 days" starts partway
 * through the 31st day back.
 */
export function alignWindow(
  current: CostDay[],
  prior: CostDay[] | null,
  days: number,
  now: Date = new Date()
): AlignedDay[] {
  const byDay = new Map(current.map((d) => [d.day, d]));
  const priorByDay = prior ? new Map(prior.map((d) => [d.day, d])) : null;
  const today = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());
  const out: AlignedDay[] = [];
  for (let i = days; i >= 0; i -= 1) {
    const t = today - i * DAY_MS;
    const day = isoDay(t);
    const d = byDay.get(day);
    out.push({
      day,
      cost_usd: d?.cost_usd ?? 0,
      run_count: d?.run_count ?? 0,
      prior_cost_usd: priorByDay ? (priorByDay.get(isoDay(t - days * DAY_MS))?.cost_usd ?? 0) : null
    });
  }
  return out;
}
