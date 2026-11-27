import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import { RUN_CLOCK, startDateAt } from "./page-clock.js";

const AT = Date.UTC(2026, 10, 25, 9, 14);

/** `startDateAt` as a page gets it, as source text, then `probe` evaluated in that page. */
function inAPage<T>(probe: string, globals: Record<string, unknown> = {}): T {
  return runInNewContext(`(${startDateAt.toString()})(${AT}); ${probe}`, globals) as T;
}

describe("startDateAt", () => {
  it("starts every reading of now at the run's instant", () => {
    const [now, made] = inAPage<[number, number]>("[Date.now(), new Date().getTime()]");
    expect(now - AT).toBeGreaterThanOrEqual(0);
    expect(now - AT).toBeLessThan(1_000);
    expect(made - AT).toBeLessThan(1_000);
    expect(inAPage<string>("Date()").slice(0, 21)).toBe(
      runInNewContext(`new Date(${AT}).toString()`).slice(0, 21)
    );
  });

  // A clock that stood still kept `Date.now() - since >= wait` false for ever,
  // so a debounced first load never fired (review finding F20).
  it("runs: elapsed time is measured as ever, and a debounce fires", async () => {
    // Bounded, so a clock that stands still fails this rather than hanging it.
    const waited = inAPage<number>(
      "const a = Date.now(); for (let n = 0; Date.now() - a < 25 && n < 2e8; n++) {} Date.now() - a"
    );
    expect(waited).toBeGreaterThanOrEqual(25);
    const fired = await new Promise<boolean>((resolve) => {
      inAPage(
        `const since = Date.now();
         (function tick() { if (Date.now() - since >= 30) done(true); else setTimeout(tick, 5); })();`,
        { setTimeout, done: resolve }
      );
      setTimeout(() => resolve(false), 1_000);
    });
    expect(fired).toBe(true);
  });

  it("leaves every other date as it was, and a Date a Date", () => {
    expect(inAPage("new Date(5).getTime()")).toBe(5);
    expect(inAPage('new Date("2026-01-02T03:04:05Z").toISOString()')).toBe(
      "2026-01-02T03:04:05.000Z"
    );
    expect(inAPage("new Date(2026, 0, 2).getFullYear()")).toBe(2026);
    expect(inAPage('Date.parse("2026-01-02T00:00:00Z")')).toBe(Date.UTC(2026, 0, 2));
    expect(inAPage("Date.UTC(2026, 0, 2)")).toBe(Date.UTC(2026, 0, 2));
    expect(inAPage("new Date() instanceof Date")).toBe(true);
    expect(inAPage("typeof new Date().toLocaleTimeString()")).toBe("string");
  });

  it("starts on a whole minute, so pages opened seconds apart draw the same one", () => {
    expect(RUN_CLOCK % 60_000).toBe(0);
    expect(Date.now() - RUN_CLOCK).toBeLessThan(10 * 60_000);
  });
});
