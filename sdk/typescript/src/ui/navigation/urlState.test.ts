// @vitest-environment jsdom
//
// A screen's selections in the query string, against jsdom's real session history: a param
// round trip, null and empty delete, no new history entry, the entry's state kept (a phone
// sheet's depth survives a write), and the phone's Back across a sheet entry keeps what the
// screen last wrote. Unreadable params are absent, never an error.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { parseMonthParam, parseOneOf, readUrlParam, writeUrlParams } from "./urlState";

/** Back: jsdom fires popstate on a later task. */
const back = () =>
  new Promise<void>((resolve) => {
    window.addEventListener("popstate", () => setTimeout(resolve, 0), { once: true });
    window.history.back();
  });

beforeEach(() => {
  window.history.replaceState(null, "", "/a/bookings/flags");
});

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

describe("readUrlParam / writeUrlParams", () => {
  it("round-trips a param, merges with the others, and deletes on null or empty", () => {
    writeUrlParams({ metric: "food_cost", month: "2026-07" });
    expect(readUrlParam("metric")).toBe("food_cost");
    expect(readUrlParam("month")).toBe("2026-07");
    writeUrlParams({ metric: "wage cost & more" });
    expect(readUrlParam("metric")).toBe("wage cost & more");
    expect(readUrlParam("month")).toBe("2026-07");
    writeUrlParams({ metric: null, month: "" });
    expect(window.location.search).toBe("");
    expect(readUrlParam("metric")).toBeNull();
    expect(window.location.pathname).toBe("/a/bookings/flags");
  });

  it("adds no history entry and keeps the entry's own state", () => {
    window.history.replaceState({ oxyPhoneSheetDepth: 1 }, "", window.location.href);
    const before = window.history.length;
    writeUrlParams({ sort: "name" });
    expect(window.history.length).toBe(before);
    expect(window.history.state).toEqual({ oxyPhoneSheetDepth: 1 });
  });

  it("keeps a filter picked in a phone sheet across the phone's Back", async () => {
    writeUrlParams({ metric: null }); // the screen mounts
    // The kit opens a filter sheet: an entry of its own on the same URL.
    window.history.pushState({ oxyPhoneSheetDepth: 1 }, "", window.location.href);
    writeUrlParams({ metric: "food_cost" });
    await back();
    expect(window.location.search).toBe("?metric=food_cost");
    expect(window.history.state).toBeNull();
  });
});

describe("the parsers: a URL is untrusted input", () => {
  it("accepts a real month and nothing else", () => {
    expect(parseMonthParam("2026-07")).toBe("2026-07");
    for (const bad of ["2026-13", "2026-00", "2026-7", "July", "", null])
      expect(parseMonthParam(bad)).toBeUndefined();
  });

  it("falls back rather than filtering to nothing", () => {
    const metrics = ["all", "food_cost", "wage_cost"] as const;
    expect(parseOneOf("wage_cost", metrics, "all")).toBe("wage_cost");
    expect(parseOneOf("cogs", metrics, "all")).toBe("all");
    expect(parseOneOf(null, metrics, "all")).toBe("all");
  });
});
