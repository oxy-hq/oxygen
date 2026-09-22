// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest";
import type { BoundKioskDevice } from "@/types/frontline";
import { readRecentCrew, recentCrewKey, rememberCrewSignIn } from "./recentCrew";

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

const kiosk = (overrides: Partial<BoundKioskDevice> = {}): BoundKioskDevice => ({
  bound: true,
  org: "poke",
  orgName: "Poke",
  device: "Front counter",
  location: { id: "loc-clovis", name: "Clovis" },
  returnTo: null,
  ...overrides
});

describe("rememberCrewSignIn", () => {
  it("keeps the newest sign-in first, and a returning worker only once", () => {
    const device = kiosk();
    for (const id of ["ana", "ben", "cy", "ana"]) {
      rememberCrewSignIn(device, id);
    }
    expect(readRecentCrew(device)).toEqual(["ana", "cy", "ben"]);
  });

  it("forgets the oldest past eight, so the list never grows with the staff turnover", () => {
    const device = kiosk();
    for (let i = 0; i < 12; i++) {
      rememberCrewSignIn(device, `id-${i}`);
    }
    expect(readRecentCrew(device)).toEqual([
      "id-11",
      "id-10",
      "id-9",
      "id-8",
      "id-7",
      "id-6",
      "id-5",
      "id-4"
    ]);
  });

  it("is this kiosk's own: another org, store or tablet reads nothing of it", () => {
    rememberCrewSignIn(kiosk(), "ana");
    expect(readRecentCrew(kiosk({ org: "other" }))).toEqual([]);
    expect(readRecentCrew(kiosk({ location: { id: "loc-fresno", name: "Fresno" } }))).toEqual([]);
    expect(readRecentCrew(kiosk({ device: "Back office" }))).toEqual([]);
    expect(readRecentCrew(kiosk())).toEqual(["ana"]);
  });

  it("stores identifiers and nothing else", () => {
    const device = kiosk();
    rememberCrewSignIn(device, "ana");
    expect(JSON.parse(localStorage.getItem(recentCrewKey(device)) ?? "null")).toEqual(["ana"]);
  });
});

describe("readRecentCrew", () => {
  it("reads a corrupted or foreign value as nobody, not as a crash", () => {
    const device = kiosk();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    for (const raw of ["not json", '{"ana":1}', "[1,2]", "null"]) {
      localStorage.setItem(recentCrewKey(device), raw);
      expect(readRecentCrew(device)).toEqual([]);
    }
  });

  it("carries on without the row when the browser refuses storage", () => {
    // A locked-down kiosk browser, or private mode: the picker still works.
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("SecurityError");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("QuotaExceededError");
    });
    expect(readRecentCrew(kiosk())).toEqual([]);
    expect(() => rememberCrewSignIn(kiosk(), "ana")).not.toThrow();
    expect(warn).toHaveBeenCalled();
  });
});
