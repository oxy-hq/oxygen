import { describe, expect, it } from "vitest";
import {
  complianceLabel,
  effectFormat,
  formatAlpha,
  formatGroupDay,
  formatInterval,
  formatMoney,
  formatMoneyInterval,
  formatRatio,
  formatRatioInterval,
  formatShare,
  formatSignedMoney,
  formatSignedUnitDay,
  formatUnitDay,
  leakLabel,
  NOT_AVAILABLE
} from "./format";

describe("formatUnitDay", () => {
  it("prices per unit-day with grouping and two decimals", () => {
    expect(formatUnitDay(17.5)).toBe("$17.50/unit-day");
    expect(formatUnitDay(1234.5)).toBe("$1,234.50/unit-day");
    expect(formatUnitDay(-3.1)).toBe("-$3.10/unit-day");
  });

  it("renders a missing or non-finite number as a dash, never as $0", () => {
    expect(formatUnitDay(null)).toBe(NOT_AVAILABLE);
    expect(formatUnitDay(Number.NaN)).toBe(NOT_AVAILABLE);
    expect(formatUnitDay(Number.POSITIVE_INFINITY)).toBe(NOT_AVAILABLE);
  });
});

describe("formatSignedUnitDay / formatSignedMoney", () => {
  it("signs a positive and a negative effect, and leaves zero unsigned", () => {
    expect(formatSignedUnitDay(17.5)).toBe("+$17.50/unit-day");
    expect(formatSignedUnitDay(-3.1)).toBe("-$3.10/unit-day");
    expect(formatSignedUnitDay(0)).toBe("$0.00/unit-day");
    expect(formatSignedMoney(3057.6)).toBe("+$3,057.60");
    expect(formatSignedMoney(-20)).toBe("-$20.00");
    expect(formatSignedMoney(null)).toBe(NOT_AVAILABLE);
  });
});

describe("formatMoney", () => {
  it("leaves a positive amount unsigned and a missing one a dash", () => {
    expect(formatMoney(1234.5)).toBe("$1,234.50");
    expect(formatMoney(-7)).toBe("-$7.00");
    expect(formatMoney(null)).toBe(NOT_AVAILABLE);
    expect(formatMoney(Number.NaN)).toBe(NOT_AVAILABLE);
  });
});

describe("complianceLabel", () => {
  it("complianceLabel_reports_not_measured_when_no_treated_entries", () => {
    // n_treated 0 means nothing was read, so c_treated is a 0 that measured nothing, not 0%.
    expect(complianceLabel({ n_treated: 0, c_treated: 0 })).toEqual({
      measured: false,
      text: "Compliance not measured"
    });
  });

  it("shows the share once at least one treated entry was read", () => {
    expect(complianceLabel({ n_treated: 4, c_treated: 0.5 })).toEqual({
      measured: true,
      text: "50.0%"
    });
    expect(complianceLabel({ n_treated: 4, c_treated: 0 })).toEqual({
      measured: true,
      text: "0.0%"
    });
  });
});

describe("leakLabel", () => {
  it("is not measured when no control entry was read, rather than 0%", () => {
    expect(leakLabel({ n_control: 0, c_control_leak: 0 })).toBe("not measured");
  });

  it("shows the share once a control entry was read, including a real 0%", () => {
    expect(leakLabel({ n_control: 4, c_control_leak: 0.1 })).toBe("10.0%");
    expect(leakLabel({ n_control: 4, c_control_leak: 0 })).toBe("0.0%");
  });
});

describe("formatShare / formatRatio", () => {
  it("renders a fraction as a one-decimal percent", () => {
    expect(formatShare(0.674)).toBe("67.4%");
    expect(formatShare(1)).toBe("100.0%");
    expect(formatShare(null)).toBe(NOT_AVAILABLE);
  });

  it("renders a slope to four significant figures with no currency", () => {
    expect(formatRatio(1.42)).toBe("1.42");
    expect(formatRatio(-0.000123456)).toBe("-0.0001235");
    expect(formatRatio(null)).toBe(NOT_AVAILABLE);
  });
});

describe("intervals", () => {
  it("renders a bounded interval with signed endpoints", () => {
    expect(formatInterval(-2, 4)).toBe("-$2.00/unit-day to +$4.00/unit-day");
    expect(formatMoneyInterval(100, 250)).toBe("+$100.00 to +$250.00");
    expect(formatRatioInterval(0.5, 2)).toBe("0.5 to 2");
  });

  it("renders a null interval endpoint as unbounded, never as $0", () => {
    expect(formatInterval(null, null)).toBe("unbounded below to unbounded above");
    expect(formatInterval(1.5, null)).toBe("+$1.50/unit-day to unbounded above");
    expect(formatInterval(null, 3)).toBe("unbounded below to +$3.00/unit-day");
    expect(formatInterval(null, null)).not.toContain("$0");
    expect(formatMoneyInterval(null, null)).toBe("unbounded below to unbounded above");
    expect(formatRatioInterval(null, 2)).toBe("unbounded below to 2");
  });

  it("treats an infinite endpoint the same as a null one", () => {
    expect(formatInterval(Number.NEGATIVE_INFINITY, 3)).toBe("unbounded below to +$3.00/unit-day");
  });
});

describe("formatGroupDay", () => {
  it("prices per group-day, the unit a grouped design's own MDE is in", () => {
    expect(formatGroupDay(1200)).toBe("$1,200.00/group-day");
    expect(formatGroupDay(null)).toBe(NOT_AVAILABLE);
  });
});

describe("formatAlpha", () => {
  it("prints α as a bare number without trailing zeros or float noise", () => {
    expect(formatAlpha(0.05)).toBe("0.05");
    expect(formatAlpha(0.2)).toBe("0.2");
    expect(formatAlpha(0.1 + 0.1)).toBe("0.2");
    expect(formatAlpha(null)).toBe(NOT_AVAILABLE);
  });
});

describe("effectFormat", () => {
  it("prices a groups design per group-day and every other design per unit-day", () => {
    expect(effectFormat("groups").signed(18.2)).toBe("+$18.20/group-day");
    expect(effectFormat("groups").plain(86)).toBe("$86.00/group-day");
    expect(effectFormat("groups").interval(null, 3)).toBe("unbounded below to +$3.00/group-day");
    expect(effectFormat("units").signed(18.2)).toBe("+$18.20/unit-day");
    expect(effectFormat("switchback").interval(1, 2)).toBe("+$1.00/unit-day to +$2.00/unit-day");
  });

  it("falls back to per unit-day for a design this build does not know, and never prints a missing number as $0", () => {
    expect(effectFormat("something-new").plain(18.2)).toBe("$18.20/unit-day");
    expect(effectFormat("groups").signed(null)).toBe(NOT_AVAILABLE);
  });
});

describe("effectFormat for a share", () => {
  it("prints a ratio outcome as a plain change per unit, never as money per unit-day", () => {
    const fmt = effectFormat("units", "day", { share: true });
    expect(fmt.signed(0.12)).toBe("+12.0 pp per unit");
    expect(fmt.plain(-0.031)).toBe("-3.1 pp per unit");
    expect(fmt.interval(0.11, 0.13)).toBe("+11.0 pp per unit to +13.0 pp per unit");
    expect(fmt.signed(0.004)).toBe("+0.4 pp per unit");
  });

  it("puts no sign on a share change that rounds to zero", () => {
    const fmt = effectFormat("units", "day", { share: true });
    expect(fmt.signed(0.0004)).toBe("0.0 pp per unit");
    expect(fmt.signed(-0.0004)).toBe("0.0 pp per unit");
  });
});
