// @vitest-environment jsdom
//
// The freshness stamp: its three wordings at a fixed `now` (the format entry's words), the feed
// behind said in words, nothing drawn for a stamp that cannot be read, a machine-readable
// <time>, and the live clock that ticks only while the page is visible.

import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ui } from "../recipes";
import { $, fire, render } from "../ui.harness";
import { AsOf, asOfText } from "./AsOf";

const NOW = "2026-11-19T15:06:00Z";

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
  setVisibility("visible");
});

function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state });
}

const stamp = () => $("time") as HTMLTimeElement | null;

/** Time passes, and React renders what it set off. */
const pass = (ms: number) =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });

describe("AsOf wording", () => {
  it("says As of the time today, and names the day and year otherwise", () => {
    // The zone is passed: with none, a browser reads the viewer's own (decision 65).
    expect(asOfText("2026-11-19T15:03:00Z", "asOf", NOW, "UTC")).toBe("As of 3:03 PM");
    expect(asOfText("2026-11-18T15:03:00Z", "asOf", NOW, "UTC")).toBe("As of Nov 18, 3:03 PM");
    expect(asOfText("2025-11-18T15:03:00Z", "asOf", NOW, "UTC")).toBe(
      "As of Nov 18, 2025, 3:03 PM"
    );
    expect(
      asOfText("2026-11-19T07:30:00Z", "asOf", "2026-11-19T08:30:00Z", "America/Los_Angeles")
    ).toBe("As of Nov 18, 11:30 PM");
  });

  it("says Updated, how long ago", () => {
    expect(asOfText("2026-11-19T15:05:40Z", "updated", NOW)).toBe("Updated just now");
    expect(asOfText("2026-11-19T15:03:00Z", "updated", NOW)).toBe("Updated 3m ago");
    expect(asOfText("2026-11-19T12:00:00Z", "updated", NOW)).toBe("Updated 3h ago");
    expect(asOfText("2026-11-18T12:00:00Z", "updated", NOW)).toBe("Updated yesterday");
    expect(asOfText("2026-11-02T12:00:00Z", "updated", NOW)).toBe("Updated Nov 2");
  });

  it("says Data through the newest day", () => {
    expect(asOfText("2026-11-18", "dataThrough", NOW)).toBe("Data through Nov 18");
    expect(asOfText("2025-12-31", "dataThrough", NOW)).toBe("Data through Dec 31, 2025");
  });

  it("has no words for a stamp it cannot read", () => {
    for (const at of [null, undefined, "", "not a date"]) {
      expect(asOfText(at, "asOf", NOW)).toBeNull();
      expect(asOfText(at, "updated", NOW)).toBeNull();
      expect(asOfText(at, "dataThrough", NOW)).toBeNull();
    }
  });
});

describe("AsOf", () => {
  it("is a note in a <time> that names the instant", async () => {
    await render(<AsOf at='2026-11-19T15:03:00Z' now={NOW} timeZone='UTC' />);
    const t = stamp();
    expect(t?.textContent).toBe("As of 3:03 PM");
    expect(t?.getAttribute("datetime")).toBe("2026-11-19T15:03:00Z");
    expect(t?.getAttribute("title")).toBe("Nov 19, 2026, 3:03 PM");
    expect(t?.className).toContain(ui.note);
  });

  it("says the feed is behind in words and in the warning ink", async () => {
    const page = await render(<AsOf at='2026-11-16' kind='dataThrough' now={NOW} behind />);
    expect(stamp()?.textContent).toBe("Feed behind · Data through Nov 16");
    expect(stamp()?.className).toContain(ui.toneText.warning);
    await page.rerender(<AsOf at='2026-11-18' kind='dataThrough' now={NOW} />);
    expect(stamp()?.textContent).toBe("Data through Nov 18");
    expect(stamp()?.className).not.toContain(ui.toneText.warning);
  });

  it("draws nothing for a stamp it cannot read", async () => {
    const page = await render(<AsOf at={null} now={NOW} />);
    expect(stamp()).toBeNull();
    await page.rerender(<AsOf at='soon' kind='updated' now={NOW} />);
    expect(stamp()).toBeNull();
  });

  it("keeps its own time, re-read on a timer only while the page is visible", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    vi.setSystemTime(new Date("2026-11-19T15:06:00Z"));
    await render(<AsOf at='2026-11-19T15:03:00Z' kind='updated' />);
    expect(stamp()?.textContent).toBe("Updated 3m ago");
    await pass(60_000);
    expect(stamp()?.textContent).toBe("Updated 4m ago");

    setVisibility("hidden");
    await fire(document as unknown as Element, new Event("visibilitychange"));
    await pass(5 * 60_000);
    expect(stamp()?.textContent, "a hidden page does not tick").toBe("Updated 4m ago");
    expect(vi.getTimerCount(), "the timer stops while hidden").toBe(0);

    setVisibility("visible");
    await fire(document as unknown as Element, new Event("visibilitychange"));
    await pass(0);
    expect(stamp()?.textContent, "and catches up the moment it shows").toBe("Updated 9m ago");
  });

  it("holds still with a fixed now, and sets no timer", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    vi.setSystemTime(new Date("2026-11-19T15:06:00Z"));
    await render(<AsOf at='2026-11-19T15:03:00Z' kind='updated' now={NOW} />);
    expect(vi.getTimerCount()).toBe(0);
    await pass(10 * 60_000);
    expect(stamp()?.textContent).toBe("Updated 3m ago");
  });
});
