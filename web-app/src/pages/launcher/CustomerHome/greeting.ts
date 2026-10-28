/** "Good morning" / "Good afternoon" / "Good evening" for the hour of `now`. */
export function partOfDayGreeting(now: Date = new Date()): string {
  const hour = now.getHours();
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

/** "Good afternoon, Priya", or "Good afternoon" without a usable name. */
export function homeGreeting(name: string | null | undefined, now: Date = new Date()): string {
  const first = firstName(name);
  const greeting = partOfDayGreeting(now);
  return first ? `${greeting}, ${first}` : greeting;
}
