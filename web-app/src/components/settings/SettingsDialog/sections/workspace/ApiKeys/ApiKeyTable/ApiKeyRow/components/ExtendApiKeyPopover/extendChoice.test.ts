// @vitest-environment jsdom
// jsdom because ApiKeyService's module pulls in the API client, which reads `window.location`.
import { describe, expect, it } from "vitest";
import {
  buildExtendRequest,
  endOfLocalDay,
  firstPickableDay,
  previewExpiry,
  submitLabel
} from "./extendChoice";

const NOW = new Date("2026-10-01T12:00:00Z");
const DAY = 24 * 60 * 60 * 1000;

describe("previewExpiry", () => {
  it("counts days from the current expiry when it is still ahead", () => {
    const key = { expires_at: "2026-10-11T12:00:00Z" };
    const got = previewExpiry(key, { kind: "days", days: 30 }, NOW);
    expect(got?.toISOString()).toBe(new Date(Date.parse(key.expires_at) + 30 * DAY).toISOString());
  });

  it("counts days from now for a key that already lapsed, so a revival gets the full term", () => {
    const key = { expires_at: "2026-09-01T00:00:00Z" };
    const got = previewExpiry(key, { kind: "days", days: 90 }, NOW);
    expect(got?.toISOString()).toBe(new Date(NOW.getTime() + 90 * DAY).toISOString());
  });

  it("is null for no expiry and undefined while no date is picked", () => {
    expect(previewExpiry({}, { kind: "never" }, NOW)).toBeNull();
    expect(previewExpiry({}, { kind: "date" }, NOW)).toBeUndefined();
  });
});

describe("buildExtendRequest", () => {
  it("sends exactly one of the contract's body shapes", () => {
    expect(buildExtendRequest({ kind: "days", days: 365 })).toEqual({ days: 365 });
    expect(buildExtendRequest({ kind: "never" })).toEqual({ expires_at: null });
    const date = new Date(2027, 0, 31);
    expect(buildExtendRequest({ kind: "date", date })).toEqual({
      expires_at: endOfLocalDay(date).toISOString()
    });
  });

  it("has no body until a date is picked", () => {
    expect(buildExtendRequest({ kind: "date" })).toBeUndefined();
  });
});

describe("date picker bounds", () => {
  it("starts at local midnight tomorrow, so the picked expiry is always in the future", () => {
    const first = firstPickableDay(new Date(2026, 9, 1, 23, 30));
    expect([first.getFullYear(), first.getMonth(), first.getDate(), first.getHours()]).toEqual([
      2026, 9, 2, 0
    ]);
  });

  it("expires at the end of the picked day", () => {
    const end = endOfLocalDay(new Date(2027, 0, 31, 8));
    expect([end.getDate(), end.getHours(), end.getMinutes()]).toEqual([31, 23, 59]);
  });
});

describe("submitLabel", () => {
  it("names the outcome", () => {
    expect(submitLabel(undefined)).toBe("Pick a date");
    expect(submitLabel(null)).toBe("Remove expiry");
    expect(submitLabel(new Date(2027, 0, 30, 12))).toBe("Extend to Jan 30, 2027");
  });
});
