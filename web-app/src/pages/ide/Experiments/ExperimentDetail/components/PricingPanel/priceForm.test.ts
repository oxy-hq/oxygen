import { describe, expect, it } from "vitest";
import {
  type CandidateFormValues,
  expandedCount,
  MAX_CANDIDATES,
  newCandidate,
  type PriceFormContext,
  validatePriceForm
} from "./priceForm";

const ctx: PriceFormContext = { earliestToday: "2026-09-29" };

const waves = (over: Partial<CandidateFormValues> = {}): CandidateFormValues => ({
  ...newCandidate("waves"),
  waveSizes: "3, 3",
  firstSwitchOn: "2026-10-05",
  ...over
});
const swb = (over: Partial<CandidateFormValues> = {}): CandidateFormValues => ({
  ...newCandidate("switchback"),
  firstPeriodOn: "2026-10-05",
  ...over
});
const check = (rows: CandidateFormValues[], context = ctx) =>
  validatePriceForm({ candidates: rows }, context);
const wavesErrors = (over: Partial<CandidateFormValues>) => check([waves(over)]).candidateErrors[0];

describe("validatePriceForm — one waves candidate", () => {
  it("builds the request from a valid row, with the α stated explicitly", () => {
    expect(check([waves()])).toEqual({
      candidateErrors: [{}],
      formError: null,
      request: {
        candidates: [
          {
            shape: "waves",
            wave_sizes: [3, 3],
            first_switch_on: "2026-10-05",
            spacing_days: 14,
            alpha: 0.05
          }
        ]
      }
    });
  });

  it("reports every empty field at once, next to its own field, and builds no request", () => {
    const { candidateErrors, request } = check([
      waves({ waveSizes: "", firstSwitchOn: "", spacingDays: "" })
    ]);
    expect(candidateErrors[0]).toEqual({
      waveSizes: "Enter at least one wave size, for example 2, 3.",
      firstSwitchOn: "Choose the first switch date.",
      spacingDays: "Enter the spacing between waves in days."
    });
    expect(request).toBeNull();
  });

  it.each([
    ["0", "A wave size of 0 places no units. Use 2 or more."],
    ["1, 3", "Wave sizes must be 2 or more; 1 is too small."],
    ["3, 0", "A wave size of 0 places no units. Use 2 or more."],
    ["2,,3", "Wave sizes has an empty entry. Separate sizes with single commas, for example 2, 3."],
    [
      "2, 3,",
      "Wave sizes has an empty entry. Separate sizes with single commas, for example 2, 3."
    ],
    ["2, x", '"x" is not a whole number of 2 or more.'],
    ["-3", '"-3" is not a whole number of 2 or more.'],
    ["2.5", '"2.5" is not a whole number of 2 or more.']
  ])("wave sizes %j -> %s", (waveSizes, message) => {
    expect(wavesErrors({ waveSizes }).waveSizes).toBe(message);
    expect(check([waves({ waveSizes })]).request).toBeNull();
  });

  it("accepts a single wave and whitespace around entries", () => {
    const sizes = (waveSizes: string) => {
      const candidate = check([waves({ waveSizes })]).request?.candidates[0];
      return candidate?.shape === "waves" ? candidate.wave_sizes : undefined;
    };
    expect(sizes(" 4 ")).toEqual([4]);
    expect(sizes("2 ,  5")).toEqual([2, 5]);
  });

  it("refuses a first switch date that is past in every time zone, and names the cut-off", () => {
    expect(wavesErrors({ firstSwitchOn: "2026-09-28" }).firstSwitchOn).toBe(
      "2026-09-28 is already past everywhere (the earliest date on Earth today is 2026-09-29). Choose a later date."
    );
  });

  it("allows the earliest date on Earth and refuses a date that is not on the calendar", () => {
    expect(wavesErrors({ firstSwitchOn: "2026-09-29" }).firstSwitchOn).toBeUndefined();
    expect(wavesErrors({ firstSwitchOn: "2026-02-31" }).firstSwitchOn).toBe(
      "Enter a real calendar date as YYYY-MM-DD."
    );
  });

  it.each(["0", "1.5", "-2", "x"])("spacing %j is refused", (spacingDays) => {
    expect(wavesErrors({ spacingDays }).spacingDays).toBe(
      "Spacing must be a whole number of days, 1 or more."
    );
  });
});

describe("validatePriceForm — the server's upper bounds", () => {
  it("refuses more than 52 waves, and accepts exactly 52", () => {
    const sizes = (n: number) => Array(n).fill("2").join(", ");
    expect(wavesErrors({ waveSizes: sizes(53) }).waveSizes).toBe("At most 52 waves, got 53.");
    expect(wavesErrors({ waveSizes: sizes(52) })).toEqual({});
  });

  it("refuses spacing over 366 days when there is more than one wave", () => {
    expect(wavesErrors({ spacingDays: "367" }).spacingDays).toBe(
      "Spacing must be between 1 and 366 days when there is more than one wave."
    );
    expect(wavesErrors({ spacingDays: "366" })).toEqual({});
    // One wave has no spacing: the server does not check it.
    expect(wavesErrors({ waveSizes: "3", spacingDays: "400" })).toEqual({});
  });

  it("refuses a period over 56 days and more than 26 pairs", () => {
    const e = check([swb({ periodDays: "57", pairs: "27" })]).candidateErrors[0];
    expect(e.periodDays).toBe("Period length must be between 1 and 56 days.");
    expect(e.pairs).toBe("Pairs must be between 1 and 26.");
    expect(check([swb({ periodDays: "56", pairs: "26" })]).candidateErrors[0]).toEqual({});
  });
});

describe("validatePriceForm — blocks and groups", () => {
  const first = (rows: CandidateFormValues[]) => check(rows).request?.candidates[0];

  it("omits blocks and assign_by when they are empty", () => {
    expect(first([waves()])).not.toHaveProperty("blocks");
    expect(first([waves()])).not.toHaveProperty("assign_by");
  });

  it("sends blocks and assign_by when given", () => {
    expect(first([waves({ blocks: "3", assignBy: " category " })])).toMatchObject({
      blocks: 3,
      assign_by: "category"
    });
  });

  it.each(["1", "0", "x", "2.5", "-2"])("blocks %j is refused", (blocks) => {
    expect(wavesErrors({ blocks }).blocks).toBe(
      "Blocks must be a whole number of 2 or more, or empty for an unblocked draw."
    );
  });

  it.each(["bad name", "9lives", "a-b", "a.b"])(
    "assign_by %j is not a dimension name",
    (assignBy) => {
      expect(wavesErrors({ assignBy }).assignBy).toBe(
        "A dimension name uses letters, digits and underscores, and starts with a letter."
      );
    }
  );
});

describe("validatePriceForm — switchback", () => {
  it("builds a switchback candidate with the α stated explicitly", () => {
    expect(check([swb()]).request).toEqual({
      candidates: [
        {
          shape: "switchback",
          first_period_on: "2026-10-05",
          period_days: 7,
          pairs: 6,
          alpha: 0.05
        }
      ]
    });
  });

  it("reports empty and out-of-range fields next to their own field", () => {
    const { candidateErrors, request } = check([
      swb({ firstPeriodOn: "", periodDays: "", pairs: "" })
    ]);
    expect(candidateErrors[0]).toEqual({
      firstPeriodOn: "Choose the first period's start date.",
      periodDays: "Enter the period length in days.",
      pairs: "Enter the number of on/off pairs."
    });
    expect(request).toBeNull();
  });

  it.each(["0", "-1", "1.5", "x"])("pairs %j is refused as not a whole number", (pairs) => {
    expect(check([swb({ pairs })]).candidateErrors[0].pairs).toBe(
      "Pairs must be a whole number of 1 or more."
    );
  });

  // A sign-flip test over n pairs has 2^n arrangements, so its smallest two-sided
  // p-value is 2/2^n: 6 pairs reach 0.05 and 4 reach 0.2, fewer never can.
  it.each([
    ["0.05", "5", "At α 0.05 a switchback needs at least 6 pairs; 5 can never reach it."],
    ["0.2", "3", "At α 0.2 a switchback needs at least 4 pairs; 3 can never reach it."],
    ["both", "5", "At α 0.05 a switchback needs at least 6 pairs; 5 can never reach it."]
  ] as const)("at α %s, %s pairs are refused before anything is queued", (alpha, pairs, msg) => {
    const { candidateErrors, request } = check([swb({ alpha, pairs })]);
    expect(candidateErrors[0].pairs).toBe(msg);
    expect(request).toBeNull();
  });

  it.each([
    ["0.05", "6"],
    ["0.2", "4"],
    ["both", "6"]
  ] as const)("at α %s, %s pairs are enough", (alpha, pairs) => {
    expect(check([swb({ alpha, pairs })]).candidateErrors[0].pairs).toBeUndefined();
  });

  it.each(["0", "-7", "x"])("period_days %j is refused", (periodDays) => {
    expect(check([swb({ periodDays })]).candidateErrors[0].periodDays).toBe(
      "Period length must be a whole number of days, 1 or more."
    );
  });

  it("checks the period against the washout only when the page was told the washout", () => {
    const withWashout = { ...ctx, washoutDays: 7 };
    expect(check([swb({ periodDays: "7" })], withWashout).candidateErrors[0].periodDays).toBe(
      "A period must be longer than the 7-day washout."
    );
    expect(
      check([swb({ periodDays: "14" })], withWashout).candidateErrors[0].periodDays
    ).toBeUndefined();
    // Unknown washout: the client says nothing; the server's refusal arrives as a proposal.
    expect(check([swb({ periodDays: "1" })]).candidateErrors[0].periodDays).toBeUndefined();
  });
});

describe("validatePriceForm — the α preset", () => {
  it("sends the directional α for the 0.2 preset", () => {
    expect(check([waves({ alpha: "0.2" })]).request?.candidates[0]).toMatchObject({ alpha: 0.2 });
  });

  it("prices one row at both alphas as two candidates of the same design, 0.05 first", () => {
    const candidates = check([waves({ alpha: "both" })]).request?.candidates ?? [];
    expect(candidates).toHaveLength(2);
    expect(candidates.map((c) => c.alpha)).toEqual([0.05, 0.2]);
    const { alpha: _a, ...firstDesign } = candidates[0];
    const { alpha: _b, ...secondDesign } = candidates[1];
    expect(firstDesign).toEqual(secondDesign);
  });
});

describe("validatePriceForm — the candidate set", () => {
  it("keeps request order across shapes", () => {
    const shapes = check([swb(), waves(), swb({ alpha: "0.2" })]).request?.candidates.map(
      (c) => c.shape
    );
    expect(shapes).toEqual(["switchback", "waves", "switchback"]);
  });

  it("ignores the fields of the shape a row is not", () => {
    const messy = { ...waves(), pairs: "banana", periodDays: "", firstPeriodOn: "nope" };
    expect(check([messy]).request?.candidates).toHaveLength(1);
    const messySwb = { ...swb(), waveSizes: "x", spacingDays: "0", blocks: "1" };
    expect(check([messySwb]).request?.candidates).toHaveLength(1);
  });

  it("reports errors per row, in order, and builds nothing while any row is wrong", () => {
    const { candidateErrors, request } = check([waves(), waves({ waveSizes: "1" }), swb()]);
    expect(candidateErrors.map((e) => Object.keys(e))).toEqual([[], ["waveSizes"], []]);
    expect(request).toBeNull();
  });

  it("refuses an empty set", () => {
    expect(check([]).formError).toBe("Add at least one candidate to price.");
  });

  it("accepts exactly six and refuses a seventh", () => {
    expect(check(Array.from({ length: MAX_CANDIDATES }, () => waves())).formError).toBeNull();
    expect(check(Array.from({ length: MAX_CANDIDATES + 1 }, () => waves())).formError).toBe(
      "A request prices 1 to 6 candidates; this one asks for 7."
    );
  });

  it("counts a row priced at both alphas twice", () => {
    const rows = Array.from({ length: 4 }, () => waves({ alpha: "both" }));
    expect(expandedCount(rows)).toBe(8);
    const { formError, request } = check(rows);
    expect(formError).toBe(
      "A request prices 1 to 6 candidates; this one asks for 8 (a row priced at both alphas counts twice)."
    );
    expect(request).toBeNull();
    expect(
      check([waves({ alpha: "both" }), waves({ alpha: "both" }), waves({ alpha: "both" })])
        .formError
    ).toBeNull();
  });
});

describe("validatePriceForm — matched pairs", () => {
  it("sends pairs with no wave sizes or blocks, which the server sets from the unit count", () => {
    const r = check([waves({ matchPairs: true, waveSizes: "", blocks: "7" })]);
    expect(r.candidateErrors).toEqual([{}]);
    expect(r.request?.candidates).toEqual([
      {
        shape: "waves",
        pairs: true,
        wave_sizes: [],
        first_switch_on: "2026-10-05",
        spacing_days: 14,
        alpha: 0.05
      }
    ]);
  });
});

describe("validatePriceForm — the design's grain", () => {
  const week = { ...ctx, grain: "week" as const };
  it("refuses a first date that does not start a week, and words lengths in weeks", () => {
    const e = check([waves({ firstSwitchOn: "2026-10-07", spacingDays: "" })], week)
      .candidateErrors[0];
    expect(e.firstSwitchOn).toMatch(/Monday/);
    expect(e.spacingDays).toBe("Enter the spacing between waves in weeks.");
  });
  it("prices a Monday at week grain", () => {
    expect(check([waves()], week).candidateErrors[0]).toEqual({});
    expect(
      check([swb({ firstPeriodOn: "2026-10-01" })], { ...ctx, grain: "month" }).candidateErrors[0]
    ).toEqual({});
  });
});

describe("validatePriceForm — hour grain", () => {
  const hour = { ...ctx, grain: "hour" as const };
  it("takes any number of hours between switches and a switch at a whole local hour", () => {
    const at = { firstSwitchOn: "2099-03-02T14:00" };
    expect(check([waves({ spacingDays: "36", ...at })], hour).candidateErrors[0]).toEqual({});
    expect(
      check([swb({ periodDays: "4", firstPeriodOn: "2099-03-02T08:00" })], hour).candidateErrors[0]
    ).toEqual({});
  });
  it("refuses a switch that is not on the hour", () => {
    expect(
      check([waves({ firstSwitchOn: "2099-03-02T14:30" })], hour).candidateErrors[0].firstSwitchOn
    ).toMatch(/whole hour/);
  });
  it("keeps a time of day out of a day-grain switch", () => {
    expect(
      check([waves({ firstSwitchOn: "2099-03-02T14:00" })], ctx).candidateErrors[0].firstSwitchOn
    ).toMatch(/YYYY-MM-DD/);
  });
});
