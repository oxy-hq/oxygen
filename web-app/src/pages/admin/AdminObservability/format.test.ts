import { describe, expect, it } from "vitest";
import { bytes, compact, ms, outcomeLabel, pct, shortModel, surfaceLabel, usd } from "./format";

describe("null is not zero", () => {
  /**
   * The rule the whole surface rests on. The API sends `null` for a ratio over
   * an empty window and for a meter this deployment cannot answer; rendering
   * either as `0` claims a measurement that was never taken — an unmeasured
   * org drawn as an idle one, which is the exact failure the backend's store
   * doc comments name three times over.
   */
  it.each([
    ["usd", usd],
    ["compact", compact],
    ["ms", ms],
    ["pct", pct],
    ["bytes", bytes]
  ])("%s renders nullish as a dash, never a zero", (_name, fn) => {
    expect(fn(null)).toBe("—");
    expect(fn(undefined)).toBe("—");
    // And NaN/Infinity, which a division in a future caller could produce.
    expect(fn(Number.NaN)).toBe("—");
    expect(fn(Number.POSITIVE_INFINITY)).toBe("—");
  });

  /** A real zero still renders as a zero. Nothing spent is a fact. */
  it("renders a measured zero as a zero", () => {
    expect(usd(0)).toBe("$0");
    expect(compact(0)).toBe("0");
    expect(ms(0)).toBe("0 ms");
    expect(pct(0)).toBe("0%");
    expect(bytes(0)).toBe("0 B");
  });
});

describe("usd", () => {
  /** Cost here spans from fractions of a cent to thousands of dollars, so the
   *  precision follows the scale — $0.0042 is a real per-run cost. */
  it("adapts precision to scale", () => {
    expect(usd(0.0042)).toBe("$0.0042");
    expect(usd(3.21)).toBe("$3.21");
    expect(usd(1_234)).toBe("$1.2k");
    expect(usd(4_500_000)).toBe("$4.50M");
  });
});

describe("pct", () => {
  /**
   * A 0.4% failure rate must not round to "0%". The difference between no
   * failures and nearly-none is the whole signal on an error-rate column, and
   * a rounded zero reads as a perfect app.
   */
  it("keeps a decimal for a small non-zero rate", () => {
    expect(pct(0.004)).toBe("0.4%");
    expect(pct(0.0001)).toBe("0.0%");
    expect(pct(0)).toBe("0%");
  });

  it("rounds ordinary rates to whole percent", () => {
    expect(pct(0.25)).toBe("25%");
    expect(pct(1)).toBe("100%");
  });

  it("takes a digit count for the places that want one", () => {
    expect(pct(0.2567, 1)).toBe("25.7%");
  });
});

describe("ms", () => {
  /** Past a second the milliseconds stop mattering and the seconds start to. */
  it("switches to seconds past a second", () => {
    expect(ms(842)).toBe("842 ms");
    expect(ms(1_500)).toBe("1.5 s");
    expect(ms(95_000)).toBe("95 s");
  });
});

describe("bytes", () => {
  it("steps through binary units", () => {
    expect(bytes(512)).toBe("512 B");
    expect(bytes(2048)).toBe("2.0 KB");
    expect(bytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(bytes(20 * 1024 * 1024 * 1024)).toBe("20 GB");
  });
});

describe("labels", () => {
  /**
   * The stored values are wire contracts: a chat run has been stamped
   * `analytics` and an automation's `workflow` since before the renames, and
   * renaming them in storage was ruled out. Renaming them on screen is what a
   * label is for — an operator should not have to know the history to read
   * the page.
   */
  it("renames the stored source types to today's words", () => {
    expect(surfaceLabel("analytics")).toBe("Chat");
    expect(surfaceLabel("workflow")).toBe("Automations");
    expect(surfaceLabel("airway")).toBe("Airway ELT");
  });

  /** An unknown value passes through rather than vanishing: a new source type
   *  must still show its spend, under whatever it calls itself. */
  it("passes an unknown source type through", () => {
    expect(surfaceLabel("something-new")).toBe("something-new");
    expect(outcomeLabel("something-new")).toBe("something-new");
  });

  /**
   * The three failure classes get names an operator can tell apart. "empty"
   * is the implicit failure — a 200 carrying a shell that never mounted — and
   * is the one a status-code rollup would report as success, so its label has
   * to say what happened rather than echo the enum.
   */
  it("names the failure classes by what happened", () => {
    expect(outcomeLabel("error")).toBe("Server error");
    expect(outcomeLabel("empty")).toBe("Never mounted");
    expect(outcomeLabel("slow")).toBe("Too slow");
  });

  it("shortens a model id to its readable part", () => {
    expect(shortModel("us.anthropic.claude-opus-5")).toBe("claude-opus-5");
    expect(shortModel("claude-sonnet-5-5-20260514")).toBe("claude-sonnet-5-5");
    expect(shortModel("claude-haiku-5-5-latest")).toBe("claude-haiku-5-5");
  });
});
