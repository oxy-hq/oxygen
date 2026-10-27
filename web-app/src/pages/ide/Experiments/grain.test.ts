import { describe, expect, it } from "vitest";
import {
  grainChoices,
  grainOf,
  isPeriodStart,
  isSwitchMoment,
  periods,
  periodWord,
  switchInputType
} from "./grain";

describe("grain", () => {
  it("reads an absent or unknown grain as day", () => {
    expect(grainOf(undefined)).toBe("day");
    expect(grainOf("fortnight")).toBe("day");
    expect(grainOf("hour")).toBe("hour");
    expect(grainOf("week")).toBe("week");
  });

  it("knows which dates open a period", () => {
    expect(isPeriodStart("2026-10-21", "day")).toBe(true);
    expect(isPeriodStart("2026-10-19", "week")).toBe(true);
    expect(isPeriodStart("2026-10-21", "week")).toBe(false);
    expect(isPeriodStart("2026-10-01", "month")).toBe(true);
    expect(isPeriodStart("2026-10-02", "month")).toBe(false);
  });

  it("words a count in its grain", () => {
    expect(periods(1, "week")).toBe("1 week");
    expect(periods(3, "month")).toBe("3 months");
    expect(periodWord("day", { plural: true, capital: true })).toBe("Days");
  });
});

describe("grainChoices", () => {
  const dims = {
    by_view: {
      shopper_days: ["shopper_days.order_date"],
      events: ["events.at", "events.day"]
    },
    datetime: ["events.at"]
  };
  it("offers hour only when the calendar is a datetime", () => {
    expect(grainChoices("shopper_days.order_value", "", dims).map((c) => c.value)).toEqual([
      "day",
      "week",
      "month"
    ]);
    expect(grainChoices("events.count", "events.at", dims)[0].value).toBe("hour");
    expect(grainChoices("events.count", "events.day", dims)[0].value).toBe("day");
  });
  it("uses the view's only date dimension when none is named", () => {
    const one = { by_view: { events: ["events.at"] }, datetime: ["events.at"] };
    expect(grainChoices("events.count", "", one)[0].value).toBe("hour");
  });
  it("keeps hour offered when the design already has it, so the value is not lost", () => {
    expect(grainChoices("shopper_days.order_value", "", dims, "hour")[0].value).toBe("hour");
  });
});

describe("switch moments", () => {
  it("is a date below hour grain and a whole local hour at hour grain", () => {
    expect(switchInputType("hour")).toBe("datetime-local");
    expect(switchInputType("week")).toBe("date");
    expect(isSwitchMoment("2026-10-26T14:00", "hour")).toBe(true);
    expect(isSwitchMoment("2026-10-26", "hour")).toBe(true);
    expect(isSwitchMoment("2026-10-26T14:30", "hour")).toBe(false);
    expect(isSwitchMoment("2026-10-26T14:00", "day")).toBe(false);
    expect(isSwitchMoment("2026-10-26", "week")).toBe(true);
    expect(isSwitchMoment("2026-10-27", "week")).toBe(false);
  });
});
