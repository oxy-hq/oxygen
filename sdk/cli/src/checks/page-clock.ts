/**
 * One clock for every page a run opens.
 *
 * "What changed" puts staging's picture of a screen beside production's and
 * counts the pixels that differ. A screen that draws the time (the SDK shell's
 * top bar does, on every screen) differed on every attempt: the two pictures
 * are taken a minute or more apart. So every page of a run starts its `Date`
 * at one instant, the minute the run started, and the same screen draws the
 * same clock on both sides.
 *
 * The clock then runs: only its starting point moves. Code that measures
 * elapsed time with `Date` (a debounce, a "give up after N seconds") behaves
 * as it does for a person; a clock that stood still kept a debounced first
 * load from ever firing. Pictures are taken seconds after a page is ready,
 * so a clock drawn to the minute agrees.
 */

/** The instant a run's pages start from: the minute this process started checking. */
export const RUN_CLOCK = Math.floor(Date.now() / 60_000) * 60_000;

/**
 * In the page, before its own scripts: `Date` starts at `at` and runs.
 * `new Date()` and `Date.now()` give `at` plus the time since the page began;
 * `new Date(value)`, `Date.parse` and `Date.UTC` work as ever, and what it
 * makes is still a `Date`.
 *
 * Self-contained: Playwright sends it to the page as source text.
 */
export function startDateAt(at: number): void {
  const Real = Date;
  const began = Real.now();
  const now = () => at + (Real.now() - began);
  const Started = function (this: unknown, ...args: unknown[]) {
    // Called without `new`, `Date()` is the current time as a string.
    if (!new.target) return new Real(now()).toString();
    return args.length === 0
      ? new Real(now())
      : new (Real as unknown as new (...a: unknown[]) => Date)(...args);
  } as unknown as DateConstructor;
  Object.defineProperties(Started, {
    prototype: { value: Real.prototype },
    now: { value: now },
    parse: { value: Real.parse },
    UTC: { value: Real.UTC }
  });
  (globalThis as { Date: DateConstructor }).Date = Started;
}
