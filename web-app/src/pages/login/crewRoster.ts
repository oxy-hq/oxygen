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

/**
 * One tile of the name board, in the order it is drawn.
 */
export interface RosterTile {
  member: FrontlineStaff;
  /**
   * The letter this name files under — set on the FIRST name of each letter
   * only. That tile wears the letter in its corner and is where the A–Z rail
   * lands; every other tile is `null`.
   */
  letter: string | null;
}

/**
 * The roster as ONE continuous A–Z run of tiles.
 *
 * A block per letter left a half-empty row under every letter with an odd
 * count, which at 20 people is most of them, and its headings cost a row
 * each. The names now run on; the first of each letter carries the letter,
 * which is all the rail needs to jump to. Order and the `#` bucket are
 * {@link groupByInitial}'s, so the two can never disagree.
 */
export function rosterFlow(staff: FrontlineStaff[]): RosterTile[] {
  return groupByInitial(staff).flatMap((group) =>
    group.staff.map((member, i) => ({ member, letter: i === 0 ? group.letter : null }))
  );
}

/** Lower-case with accents folded, so "Álvaro" is found by "alv". */
function folded(text: string): string {
  return text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

/**
 * The names containing `query`, blind to case and accents. A blank query
 * leaves the roster whole.
 *
 * Client-side on purpose: the roster read is capped at 200 for one store and
 * is already on the tablet in full, so there is no page to fetch and no
 * reason to ask the server.
 */
export function findByName(staff: FrontlineStaff[], query: string): FrontlineStaff[] {
  const needle = folded(query.trim());
  if (!needle) {
    return staff;
  }
  return staff.filter((member) => folded(member.name).includes(needle));
}

/**
 * Past this many people the board grows a "Find your name" box. Up to it the
 * whole roster fits a tablet screen and the rail is shortcut enough; a box
 * nobody needs is one more thing between a worker and their name.
 */
const NAME_SEARCH_OVER = 30;

export function showsNameSearch(staff: FrontlineStaff[]): boolean {
  return staff.length > NAME_SEARCH_OVER;
}

/** Below this the whole roster is already on screen, so "recently" repeats it. */
const RECENT_ROW_FROM = 12;
/** One row on a tablet, two by two on a phone. */
const RECENT_ROW_SIZE = 4;

/**
 * "On this tablet recently": the people who last signed in on this kiosk,
 * newest first — only those still on today's roster, at most four.
 *
 * `recent` is what the kiosk remembered (see `recentCrew.ts`); somebody who
 * has since left the store drops out here, and the next one back takes the
 * seat. Off at a small store, where the whole roster is already in view, and
 * while someone is searching, where the row would sit above the answer.
 */
export function recentRow(
  staff: FrontlineStaff[],
  recent: string[],
  { searching }: { searching: boolean }
): FrontlineStaff[] {
  if (searching || staff.length < RECENT_ROW_FROM) {
    return [];
  }
  const byIdentifier = new Map(staff.map((member) => [member.identifier, member]));
  return recent
    .map((identifier) => byIdentifier.get(identifier))
    .filter((member): member is FrontlineStaff => member !== undefined)
    .slice(0, RECENT_ROW_SIZE);
}
