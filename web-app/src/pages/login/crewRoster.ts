import type { BoundKioskDevice, FrontlineStaff } from "@/types/frontline";

/**
 * Is this a store's tablet with nobody on its roster?
 *
 * Then nobody can sign in on it. A kiosk at a place admits exactly the people
 * its picker shows, so the ID box a roster-less kiosk falls back to could never
 * let anyone in there: every ID and right PIN typed into it would come back as
 * "that didn't match". Say why instead.
 *
 * Only a roster read that ANSWERED counts. A failed one says nothing about who
 * works here, and the ID box still signs in the store's own crew. A kiosk with
 * no place (or a server too old to report one) keeps the box, because its
 * sign-in admits the whole org.
 */
export function nobodyRosteredHere(
  device: Pick<BoundKioskDevice, "location">,
  staff: FrontlineStaff[],
  roster: { isLoading: boolean; isError: boolean }
): boolean {
  return Boolean(device.location) && !roster.isLoading && !roster.isError && staff.length === 0;
}

/**
 * One letter's worth of the name picker: the heading the A–Z rail jumps to,
 * and the names under it.
 */
export interface RosterGroup {
  /** `A`–`Z`, or `#` for a name that starts with anything else. */
  letter: string;
  staff: FrontlineStaff[];
}

/** The bucket every non-letter start shares. Last in the rail, not first. */
const OTHER = "#";

/**
 * The letter a name files under.
 *
 * Accents are folded, so "Álvaro" is under **A** rather than in `#` — the crew
 * looking for their own name are not thinking about code points. Anything that
 * is still not A–Z after folding (a digit, an emoji, an empty name) goes to
 * `#`, because a rail of one-off symbols is not an index.
 */
export function rosterInitial(name: string): string {
  // NFD splits a precomposed "Á" into "A" + a combining accent, so the first
  // code unit IS the plain letter — no character class of invisible combining
  // marks to mangle in a future diff.
  const first = name.trim().normalize("NFD").charAt(0).toUpperCase();
  return /^[A-Z]$/.test(first) ? first : OTHER;
}

/** Names as people read them: case- and accent-blind, ties broken stably. */
function byName(a: FrontlineStaff, b: FrontlineStaff): number {
  return (
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) ||
    a.identifier.localeCompare(b.identifier)
  );
}

/**
 * The roster as the picker shows it: sorted by NAME and grouped by first
 * letter.
 *
 * The server sorts by `identifier`, which is the stable login name — at Poke
 * House `jolt:<first.last>`, so it happens to read as first-name order and
 * happens to stop doing so the moment a tenant keys its people any other way.
 * A picker is scanned by the name printed on it, so the sort belongs to the
 * name; the identifier stays the tie-break so two workers called Maria keep a
 * fixed order between loads rather than swapping places under a thumb.
 */
export function groupByInitial(staff: FrontlineStaff[]): RosterGroup[] {
  const buckets = new Map<string, FrontlineStaff[]>();
  for (const member of staff) {
    const letter = rosterInitial(member.name);
    const bucket = buckets.get(letter);
    if (bucket) {
      bucket.push(member);
    } else {
      buckets.set(letter, [member]);
    }
  }
  return [...buckets.entries()]
    .sort(([a], [b]) => {
      if (a === OTHER) return 1;
      if (b === OTHER) return -1;
      return a.localeCompare(b);
    })
    .map(([letter, members]) => ({ letter, staff: [...members].sort(byName) }));
}
