import { describe, expect, it } from "vitest";
import { formatMoney, formatNumber, formatPercent, MISSING } from "./numbers";

const ABSENT = [null, undefined, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY];

describe("the missing-value mark", () => {
  it("is one em dash, for every kind of absent figure", () => {
    expect(MISSING).toBe("—");
    for (const value of ABSENT) {
      expect(formatMoney(value)).toBe(MISSING);
      expect(formatMoney(value, { compact: true, signed: true })).toBe(MISSING);
      expect(formatNumber(value)).toBe(MISSING);
      expect(formatPercent(value, { fraction: true })).toBe(MISSING);
    }
  });
});

describe("formatMoney", () => {
  it("prints whole dollars with en-US grouping by default (the reference's usd)", () => {
    expect(formatMoney(8496.4)).toBe("$8,496");
    expect(formatMoney(0)).toBe("$0");
    expect(formatMoney(999)).toBe("$999");
    expect(formatMoney(1_234_567)).toBe("$1,234,567");
  });

  it("puts a hyphen-minus before the mark, never U+2212", () => {
    const text = formatMoney(-1234.5);
    expect(text).toBe("-$1,235");
    expect(text.charCodeAt(0)).toBe(0x2d);
    expect(formatMoney(-5, { compact: true })).not.toContain("−");
  });

  it("keeps cents only when asked (the reference's usdRate)", () => {
    expect(formatMoney(12.98, { cents: true })).toBe("$12.98");
    expect(formatMoney(8.04, { cents: true })).toBe("$8.04");
    expect(formatMoney(1234.5, { cents: true })).toBe("$1,234.50");
  });

  it("rounds half away from zero on the printed decimal, not on the binary double", () => {
    expect(formatMoney(1.005, { cents: true })).toBe("$1.01");
    expect(formatMoney(2.675, { cents: true })).toBe("$2.68");
    expect(formatMoney(0.5)).toBe("$1");
    expect(formatMoney(-0.5)).toBe("-$1");
  });

  it("drops the sign of anything that rounds to zero, negative zero included", () => {
    expect(formatMoney(-0)).toBe("$0");
    expect(formatMoney(-0.4)).toBe("$0");
    expect(formatMoney(-0.004, { cents: true })).toBe("$0.00");
    expect(formatMoney(0.4, { signed: true })).toBe("$0");
  });

  it("signs a delta both ways when signed", () => {
    expect(formatMoney(1200, { signed: true })).toBe("+$1,200");
    expect(formatMoney(-1200, { signed: true })).toBe("-$1,200");
    expect(formatMoney(0, { signed: true })).toBe("$0");
    expect(formatMoney(1234, { signed: true, compact: true })).toBe("+$1.2k");
  });

  it("takes another currency mark, still before the number", () => {
    expect(formatMoney(1200, { symbol: "CA$" })).toBe("CA$1,200");
    expect(formatMoney(-5, { symbol: "€" })).toBe("-€5");
  });

  it("holds the compact bands at their boundaries, chosen after rounding", () => {
    const cases: [number, string][] = [
      [950, "$950"],
      [999.4, "$999"],
      [999.5, "$1k"],
      [1000, "$1k"],
      [1234, "$1.2k"],
      [8500, "$8.5k"],
      [9949, "$9.9k"],
      [9950, "$10k"],
      [80_000, "$80k"],
      [999_499, "$999k"],
      [999_500, "$1M"],
      [1_250_000, "$1.3M"],
      [3_400_000, "$3.4M"],
      [999_949_999, "$999.9M"],
      [999_950_000, "$1B"],
      [1_200_000_000, "$1.2B"],
      [1.5e15, "$1,500,000B"]
    ];
    for (const [value, text] of cases) {
      expect(formatMoney(value, { compact: true }), String(value)).toBe(text);
      expect(formatMoney(-value, { compact: true }), String(-value)).toBe(`-${text}`);
    }
  });

  it("prints compact amounts under $1,000 as plain money, cents included when asked", () => {
    expect(formatMoney(8.04, { compact: true, cents: true })).toBe("$8.04");
    expect(formatMoney(-0.2, { compact: true })).toBe("$0");
  });

  it("prints huge and tiny amounts in full, never in exponent notation", () => {
    expect(formatMoney(1e21)).toBe("$1,000,000,000,000,000,000,000");
    expect(formatMoney(1.5e21, { cents: true })).toBe("$1,500,000,000,000,000,000,000.00");
    expect(formatMoney(Number.MAX_VALUE)).toMatch(/^\$179,769,313,486,231,570(,000)+$/);
    expect(formatMoney(1e-7)).toBe("$0");
    expect(formatMoney(1e-7, { cents: true })).toBe("$0.00");
    expect(formatMoney(Number.MIN_VALUE, { cents: true })).toBe("$0.00");
  });
});

describe("formatNumber", () => {
  it("groups integers and counts", () => {
    expect(formatNumber(1284)).toBe("1,284");
    expect(formatNumber(-3)).toBe("-3");
    expect(formatNumber(1284.6)).toBe("1,285");
  });

  it("prints fixed decimals, clamping a bad digits value", () => {
    expect(formatNumber(12.5, { digits: 1 })).toBe("12.5");
    expect(formatNumber(0.05, { digits: 1 })).toBe("0.1");
    expect(formatNumber(1.5, { digits: -1 })).toBe("2");
    expect(formatNumber(1.234, { digits: 2.7 })).toBe("1.23");
    expect(formatNumber(1, { digits: 50 })).toBe(`1.${"0".repeat(20)}`);
  });

  it("signs a delta, and leaves a zero unsigned", () => {
    expect(formatNumber(12, { signed: true })).toBe("+12");
    expect(formatNumber(-12, { signed: true })).toBe("-12");
    expect(formatNumber(-0.4, { signed: true })).toBe("0");
    expect(formatNumber(-0)).toBe("0");
  });

  it("shares the money's compact bands", () => {
    expect(formatNumber(34_300, { compact: true })).toBe("34k");
    expect(formatNumber(1234, { compact: true })).toBe("1.2k");
    expect(formatNumber(999_500, { compact: true })).toBe("1M");
    expect(formatNumber(12.25, { compact: true, digits: 1 })).toBe("12.3");
  });
});

describe("formatPercent", () => {
  it("reads points by default, one decimal (the reference's pct)", () => {
    expect(formatPercent(28.74)).toBe("28.7%");
    expect(formatPercent(-4)).toBe("-4.0%");
    expect(formatPercent(28.5, { digits: 0 })).toBe("29%");
  });

  it("reads a fraction when told, scaling the decimal rather than multiplying", () => {
    expect(formatPercent(0.287, { fraction: true })).toBe("28.7%");
    expect(formatPercent(0.0123, { fraction: true, digits: 2 })).toBe("1.23%");
    // 0.145 * 100 is 14.499999999999998 in floating point, which would round down.
    expect(formatPercent(0.145, { fraction: true, digits: 0 })).toBe("15%");
    expect(formatPercent(1e-9, { fraction: true })).toBe("0.0%");
  });

  it("signs a change, and leaves one that rounds to zero unsigned", () => {
    expect(formatPercent(0.025, { fraction: true, signed: true })).toBe("+2.5%");
    expect(formatPercent(-2.5, { signed: true })).toBe("-2.5%");
    expect(formatPercent(-0.04, { signed: true })).toBe("0.0%");
    expect(formatPercent(-0)).toBe("0.0%");
  });

  it("does not group, as every percent copy prints", () => {
    expect(formatPercent(12_345.6)).toBe("12345.6%");
    expect(formatPercent(1e22, { digits: 0 })).toBe(`1${"0".repeat(22)}%`);
  });
});
