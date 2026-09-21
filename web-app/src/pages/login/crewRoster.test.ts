import { describe, expect, it } from "vitest";
import type { FrontlineStaff } from "@/types/frontline";
import { groupByInitial, rosterInitial } from "./crewRoster";

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
