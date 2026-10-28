import type { CustomAppSummary } from "@/types/apps";

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"}`;

/**
 * The one line under an app's name on the home that says how fresh it is.
 *
 * Today it is when the app's current version was published (`published_at`), the
 * only time the apps list carries. It says nothing about the app's data. When data
 * freshness exists (when an app's data last arrived and whether it is late, which
 * waits on #3235), it belongs here: this function is the only place the card's line
 * comes from, so that change is made here and nowhere else.
 *
 * `null` when there is nothing true to say (no time, or one that does not parse); the
 * card then shows no line rather than a made-up one.
 */
export function appFreshnessLine(
  app: Pick<CustomAppSummary, "published_at">,
  now: Date = new Date()
): string | null {
  const published = app.published_at ? new Date(app.published_at) : null;
  if (!published || Number.isNaN(published.getTime())) return null;
  const ago = now.getTime() - published.getTime();
  // A clock a little ahead of the server's reads as "just now", not as the future.
  if (ago < MINUTE_MS) return "Updated just now";
  if (ago < HOUR_MS) return `Updated ${plural(Math.floor(ago / MINUTE_MS), "minute")} ago`;
  if (ago < DAY_MS) return `Updated ${plural(Math.floor(ago / HOUR_MS), "hour")} ago`;
  const days = Math.floor(ago / DAY_MS);
  if (days === 1) return "Updated yesterday";
  if (days < 7) return `Updated ${days} days ago`;
  const sameYear = published.getFullYear() === now.getFullYear();
  return `Updated ${published.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" })
  })}`;
}
