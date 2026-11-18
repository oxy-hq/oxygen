import { describe, expect, it } from "vitest";
import {
  BASELINE_FLOOR_USD,
  bytes,
  compact,
  ms,
  outcomeLabel,
  pct,
  priorAmount,
  shortModel,
  surfaceLabel,
  usd,
  versusPrior,
  versusShort
} from "./format";

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

describe("versusPrior", () => {
  /**
   * Dev's fleet on 2026-11-18: $47 this month against $0.00000002 of storage
   * the month before. Divided through, that is "up 231432043622%" — a number
   * no row on the page can be traced to.
   */
  const DEV_TOTAL = 46.6486;
  const DEV_PRIOR = 2.0156504e-8;

  const changeOf = (v: ReturnType<typeof versusPrior>) => {
    if (v?.kind !== "pct") throw new Error(`expected a percentage, got ${v?.kind ?? "null"}`);
    return v.change;
  };

  it("draws no comparison when the prior window was not read", () => {
    expect(versusPrior(10, null)).toBeNull();
    expect(versusPrior(10, undefined)).toBeNull();
  });

  it("calls a cost over a zero prior new", () => {
    expect(versusPrior(10, 0)).toEqual({ kind: "new", prior: 0 });
  });

  it("calls a cost over a negligible prior new, never a percentage", () => {
    expect(versusPrior(DEV_TOTAL, DEV_PRIOR)).toEqual({ kind: "new", prior: DEV_PRIOR });
    expect(versusPrior(5, BASELINE_FLOOR_USD - 0.0001)?.kind).toBe("new");
    // Both windows under a cent: still nothing to divide by, not "±0%".
    expect(versusPrior(3.4272459e-9, 3.4272472e-9)?.kind).toBe("new");
  });

  it("gives a percentage over a normal prior", () => {
    expect(changeOf(versusPrior(47, 40))).toBeCloseTo(0.175);
    expect(changeOf(versusPrior(30, 40))).toBeCloseTo(-0.25);
    // A cent is a baseline: the floor itself compares.
    expect(versusPrior(0.05, BASELINE_FLOOR_USD)?.kind).toBe("pct");
  });

  it("names the baseline once the cost is ten times it", () => {
    expect(versusPrior(47, 4.7)).toEqual({ kind: "from", prior: 4.7 });
    expect(versusPrior(47, 0.02)).toEqual({ kind: "from", prior: 0.02 });
    expect(versusPrior(46.9, 4.7)?.kind).toBe("pct");
  });
});

describe("versusShort", () => {
  it("renders each kind compactly", () => {
    expect(versusShort({ kind: "new", prior: 0 })).toBe("new");
    expect(versusShort({ kind: "pct", change: 0.175 })).toBe("+18%");
    expect(versusShort({ kind: "pct", change: -0.25 })).toBe("−25%");
    expect(versusShort({ kind: "from", prior: 0.42 })).toBe("from $0.42");
  });

  /** The rule the page broke: whatever the baseline, no percentage past +899%. */
  it.each([
    [46.6486, 2.0156504e-8],
    [46.6486, 0.0099],
    [46.6486, 0.01],
    [10_000, 0.5],
    [3.4272459e-9, 3.4272472e-9]
  ])("never renders a four-digit percentage (%s against %s)", (current, prior) => {
    const v = versusPrior(current, prior);
    if (v == null) throw new Error("expected a comparison");
    expect(versusShort(v)).not.toMatch(/\d{4}%/);
  });
});

describe("priorAmount", () => {
  it("says what the prior window cost, in words a baseline can be read in", () => {
    expect(priorAmount(0)).toBe("nothing");
    expect(priorAmount(2.0156504e-8)).toBe("under $0.01");
    expect(priorAmount(0.42)).toBe("$0.42");
    expect(priorAmount(1_842.4)).toBe("$1,842");
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
