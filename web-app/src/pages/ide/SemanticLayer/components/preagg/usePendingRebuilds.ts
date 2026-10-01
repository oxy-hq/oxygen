import { useCallback, useEffect, useState } from "react";
import type { PreaggRollupStatus } from "@/services/api/semantic";

const rollupKey = (r: Pick<PreaggRollupStatus, "view_name" | "rollup_name">) =>
  `${r.view_name}.${r.rollup_name}`;

/** How often to re-read the status while something is rebuilding. */
const POLL_MS = 3_000;
/**
 * When to stop waiting on a rollup. A rebuild that fails never updates the
 * manifest, so without a deadline its row would say "Rebuilding…" forever —
 * which is the one thing worse than saying nothing. The row then falls back to
 * its real state, and the run row carries the failure.
 */
const GIVE_UP_MS = 5 * 60_000;

/**
 * The moment a rebuild last SETTLED this rollup, whichever way it settled.
 *
 * `empty_since` is in the list because a rebuild that finds zero rows retracts
 * the entry: `build_date` and `refresh_key_checked_at` both go to null, so a
 * row waiting on either would spin the full deadline and then claim the
 * rebuild never finished — against a run that did exactly what it should. The
 * cycle reports that outcome as `preagg_rollup_retracted`, counted apart from
 * "rebuilt"; this is the same distinction on the status side.
 */
const settledAtMs = (r: PreaggRollupStatus) => {
  const raw = r.refresh_key_checked_at ?? r.build_date ?? r.empty_since;
  const ms = raw ? Date.parse(raw) : Number.NaN;
  return Number.isNaN(ms) ? null : ms;
};

/**
 * Tracks which rollups are mid-rebuild.
 *
 * The server returns as soon as the work is submitted, so "done" has to be
 * observed rather than awaited: a rollup is finished when the moment it last
 * settled moves past the moment we asked for it. That's the same fact the
 * table renders, so the spinner can't disagree with the row underneath it.
 */
export function usePendingRebuilds(
  rollups: PreaggRollupStatus[],
  /** Called with the rollups that hit the deadline without settling — almost
   *  always a rebuild that failed. Without this the row would simply stop
   *  spinning and never say why. A rollup that rebuilt to zero rows does NOT
   *  reach here: it settled, via `empty_since`. */
  onGiveUp?: (keys: string[]) => void
) {
  // key → { asked: when we triggered, wasSettledAt: when it had last settled }
  const [pending, setPending] = useState<
    Record<string, { asked: number; wasSettledAt: number | null }>
  >({});

  const markPending = useCallback((targets: PreaggRollupStatus[]) => {
    const asked = Date.now();
    setPending((prev) => {
      const next = { ...prev };
      for (const r of targets) {
        next[rollupKey(r)] = { asked, wasSettledAt: settledAtMs(r) };
      }
      return next;
    });
  }, []);

  /**
   * Stop waiting on rollups whose rebuild was never actually started — a 404,
   * a 503, a 500 from the submit itself. Without this the rows spin for the
   * full GIVE_UP_MS and then fire "check the run history" for a run that was
   * never created, which sends the reader looking for a row that isn't there.
   */
  const clearPending = useCallback((targets: PreaggRollupStatus[]) => {
    setPending((prev) => {
      const next = { ...prev };
      for (const r of targets) delete next[rollupKey(r)];
      return next;
    });
  }, []);

  useEffect(() => {
    const byKey = new Map(rollups.map((r) => [rollupKey(r), r]));
    const abandoned: string[] = [];
    setPending((prev) => {
      const next: typeof prev = {};
      for (const [key, entry] of Object.entries(prev)) {
        if (Date.now() - entry.asked > GIVE_UP_MS) {
          abandoned.push(key);
          continue;
        }
        const now = byKey.get(key);
        // Gone from the config while rebuilding — nothing left to wait for.
        if (!now) continue;
        const settled = settledAtMs(now);
        const moved =
          settled !== null && (entry.wasSettledAt === null || settled > entry.wasSettledAt);
        if (!moved) next[key] = entry;
      }
      // Same identity when nothing settled, so this never loops.
      return Object.keys(next).length === Object.keys(prev).length ? prev : next;
    });
    if (abandoned.length > 0) onGiveUp?.(abandoned);
  }, [rollups, onGiveUp]);

  const keys = Object.keys(pending);
  return {
    markPending,
    clearPending,
    isPending: (r: PreaggRollupStatus) => rollupKey(r) in pending,
    anyPending: keys.length > 0,
    pollMs: keys.length > 0 ? POLL_MS : undefined
  };
}
