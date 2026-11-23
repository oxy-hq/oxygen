const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

/**
 * When a notice came, short, as the reference's panel writes it: "Just now",
 * "12m ago", "2h ago", "Yesterday", "3d ago", then the date ("Nov 5", with the
 * year once it is not this one). Empty for a time that does not parse.
 */
export function shortAgo(iso: string, now: Date = new Date()): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const ago = now.getTime() - at.getTime();
  // A clock a little ahead of the server's reads as "just now", not the future.
  if (ago < MINUTE_MS) return "Just now";
  if (ago < HOUR_MS) return `${Math.floor(ago / MINUTE_MS)}m ago`;
  if (ago < DAY_MS) return `${Math.floor(ago / HOUR_MS)}h ago`;
  if (ago < 2 * DAY_MS) return "Yesterday";
  if (ago < 7 * DAY_MS) return `${Math.floor(ago / DAY_MS)}d ago`;
  const sameYear = at.getFullYear() === now.getFullYear();
  return at.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" })
  });
}
