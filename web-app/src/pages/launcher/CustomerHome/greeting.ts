/**
 * The hour of `now` in `timeZone`, 0–23. Without a zone, or with one `Intl` does
 * not know, the browser's own hour.
 */
function hourIn(now: Date, timeZone?: string): number {
  if (!timeZone) return now.getHours();
  try {
    const hour = new Intl.DateTimeFormat("en-US", { timeZone, hour: "numeric", hourCycle: "h23" })
      .formatToParts(now)
      .find((p) => p.type === "hour")?.value;
    return hour === undefined ? now.getHours() : Number(hour) % 24;
  } catch {
    return now.getHours();
  }
}

/** "Good morning" / "Good afternoon" / "Good evening" for the hour of `now` in `timeZone`. */
function partOfDayGreeting(now: Date, timeZone?: string): string {
  const hour = hourIn(now, timeZone);
  if (hour < 12) return "Good morning";
  if (hour < 18) return "Good afternoon";
  return "Good evening";
}

/**
 * The first word of a person's name, for a greeting. `null` when the name is empty
 * or is really an email address (accounts created from an email alone carry it as
 * their name), so the greeting is "Good morning" rather than "Good morning, priya@…".
 */
export function firstName(name: string | null | undefined): string | null {
  const trimmed = name?.trim();
  if (!trimmed || trimmed.includes("@")) return null;
  return trimmed.split(/\s+/)[0] ?? null;
}

/**
 * "Good afternoon, Priya", or "Good afternoon" without a usable name. The part of
 * the day is read in `timeZone`, the workspace's, so the greeting agrees with the
 * top bar's clock rather than with the viewer's laptop.
 */
export function homeGreeting(
  name: string | null | undefined,
  now: Date = new Date(),
  timeZone?: string
): string {
  const first = firstName(name);
  const greeting = partOfDayGreeting(now, timeZone);
  return first ? `${greeting}, ${first}` : greeting;
}
