import { describe, expect, it } from "vitest";
import type { FrontlineStaff } from "@/types/frontline";
import {
  findByName,
  groupByInitial,
  recentRow,
  rosterFlow,
  rosterInitial,
  showsNameSearch
} from "./crewRoster";

const member = (name: string, identifier = name.toLowerCase()): FrontlineStaff => ({
  identifier,
  name
});

/** The groups as `["A: Ana, Abe", …]`, so a failure reads like the screen. */
const shape = (staff: FrontlineStaff[]) =>
  groupByInitial(staff).map((g) => `${g.letter}: ${g.staff.map((s) => s.name).join(", ")}`);

describe("groupByInitial", () => {
  it("groups by first letter and orders the letters A–Z", () => {
    expect(shape([member("Devon"), member("Ana"), member("Cy"), member("Abe")])).toEqual([
      "A: Abe, Ana",
      "C: Cy",
      "D: Devon"
    ]);
  });

  it("sorts by the name on the tile, not the identifier the server ordered by", () => {
    // What the roster read returns: `identifier` order, which is only ever
    // roughly name order and is not name order at all once a tenant keys its
    // people by anything but `first.last`.
    const serverOrder = [
      member("Zoe", "jolt:aaa.one"),
      member("Ana", "jolt:zzz.two"),
      member("Mia", "jolt:mmm.three")
    ];
    expect(shape(serverOrder)).toEqual(["A: Ana", "M: Mia", "Z: Zoe"]);
  });

  it("is case- and accent-blind, so nobody hides under a letter twice", () => {
    expect(shape([member("álvaro"), member("Amy"), member("ana")])).toEqual([
      "A: álvaro, Amy, ana"
    ]);
  });

  it("puts a name that starts with neither letter nor accent in # — last", () => {
    expect(shape([member("9Lives"), member("Ana"), member("2Pac")])).toEqual([
      "A: Ana",
      "#: 2Pac, 9Lives"
    ]);
  });

  it("keeps two people with the same name in a fixed order between loads", () => {
    const first = shape([member("Maria", "maria.b"), member("Maria", "maria.a")]);
    const reversed = shape([member("Maria", "maria.a"), member("Maria", "maria.b")]);
    expect(first).toEqual(reversed);
  });

  it("holds its shape at both ends of the range the picker has to work at", () => {
    // Three names: one group, and the rail has nothing to jump between.
    expect(groupByInitial([member("Ana"), member("Abe"), member("Amy")])).toHaveLength(1);

    // Two hundred: every letter present, each group non-empty, and every name
    // still on the board exactly once — the cap the server applies is per
    // store, so this is a roster a big store really can produce.
    const many = Array.from({ length: 200 }, (_, i) =>
      member(`${String.fromCharCode(65 + (i % 26))}name${i}`, `id-${i}`)
    );
    const groups = groupByInitial(many);
    expect(groups).toHaveLength(26);
    expect(groups.every((g) => g.staff.length > 0)).toBe(true);
    expect(groups.flatMap((g) => g.staff)).toHaveLength(200);
  });

  it("has nothing to show for an empty roster", () => {
    expect(groupByInitial([])).toEqual([]);
  });
});

describe("rosterInitial", () => {
  it("folds accents and refuses anything that is not a letter", () => {
    expect(rosterInitial("Álvaro")).toBe("A");
    expect(rosterInitial("  devon")).toBe("D");
    expect(rosterInitial("2Pac")).toBe("#");
    expect(rosterInitial("")).toBe("#");
  });
});

describe("rosterFlow", () => {
  /** The flow as `["A Abe", "Ana", …]`: a leading letter is the mark a tile wears. */
  const flow = (staff: FrontlineStaff[]) =>
    rosterFlow(staff).map((t) => (t.letter ? `${t.letter} ${t.member.name}` : t.member.name));

  it("runs every name on in one A–Z order, marking only the first name of each letter", () => {
    // No block per letter: a letter with one name no longer leaves half a row
    // empty. The mark is what the rail lands on.
    expect(flow([member("Devon"), member("Ana"), member("Cy"), member("Abe")])).toEqual([
      "A Abe",
      "Ana",
      "C Cy",
      "D Devon"
    ]);
  });

  it("keeps the order and the # bucket that grouping already settled", () => {
    expect(flow([member("9Lives"), member("álvaro"), member("Amy"), member("2Pac")])).toEqual([
      "A álvaro",
      "Amy",
      "# 2Pac",
      "9Lives"
    ]);
  });

  it("has nothing to show for an empty roster", () => {
    expect(rosterFlow([])).toEqual([]);
  });
});

describe("findByName", () => {
  const crew = [member("Ana Torres"), member("Álvaro Ruiz"), member("Dev Rao"), member("Devon")];
  const names = (query: string) => findByName(crew, query).map((m) => m.name);

  it("matches anywhere in the name, blind to case and accents", () => {
    expect(names("dev")).toEqual(["Dev Rao", "Devon"]);
    expect(names("RAO")).toEqual(["Dev Rao"]);
    expect(names("alv")).toEqual(["Álvaro Ruiz"]);
    expect(names("ruiz")).toEqual(["Álvaro Ruiz"]);
  });

  it("finds nobody rather than everybody for a name that is not here", () => {
    expect(names("zed")).toEqual([]);
  });

  it("leaves the roster whole for an empty or blank query", () => {
    expect(names("")).toHaveLength(4);
    expect(names("   ")).toHaveLength(4);
  });
});

describe("showsNameSearch", () => {
  const crewOf = (n: number) => Array.from({ length: n }, (_, i) => member(`Name ${i}`));

  it("offers the search box only past 30 people", () => {
    // Up to 30 the whole roster fits a tablet screen and the rail is enough.
    expect(showsNameSearch(crewOf(30))).toBe(false);
    expect(showsNameSearch(crewOf(31))).toBe(true);
  });
});

describe("recentRow", () => {
  const crewOf = (n: number) => Array.from({ length: n }, (_, i) => member(`Name ${i}`, `id-${i}`));
  const ids = (staff: FrontlineStaff[]) => staff.map((m) => m.identifier);

  it("puts the people who last signed in here first, newest first, four at most", () => {
    const recent = ["id-7", "id-2", "id-9", "id-0", "id-5"];
    expect(ids(recentRow(crewOf(20), recent, { searching: false }))).toEqual([
      "id-7",
      "id-2",
      "id-9",
      "id-0"
    ]);
  });

  it("only shows people still on this roster, and fills from further back when one has left", () => {
    // `id-99` signed in here last week and has since left the store.
    const recent = ["id-3", "id-99", "id-1", "id-4", "id-6"];
    expect(ids(recentRow(crewOf(20), recent, { searching: false }))).toEqual([
      "id-3",
      "id-1",
      "id-4",
      "id-6"
    ]);
  });

  it("stays off at a small store, where the whole roster is already in view", () => {
    expect(recentRow(crewOf(11), ["id-1"], { searching: false })).toEqual([]);
    expect(ids(recentRow(crewOf(12), ["id-1"], { searching: false }))).toEqual(["id-1"]);
  });

  it("steps aside while someone is searching", () => {
    expect(recentRow(crewOf(40), ["id-1"], { searching: true })).toEqual([]);
  });
});
