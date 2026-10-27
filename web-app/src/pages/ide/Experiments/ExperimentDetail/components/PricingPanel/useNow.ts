import { useEffect, useState } from "react";

/**
 * `Date.now()` that ticks while `active`. A refetch that returns unchanged data
 * does not re-render, so a deadline compared against a stale `now` would never
 * fire; ticking the clock is what lets "timed out" appear on its own.
 */
export function useNow(active: boolean, intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [active, intervalMs]);
  return now;
}
