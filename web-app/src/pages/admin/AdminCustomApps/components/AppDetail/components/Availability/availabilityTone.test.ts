import { describe, expect, it } from "vitest";
import type { AppAvailabilityReading } from "@/types/apps";
import {
  availabilityLabel,
  availabilityTone,
  formatObjective,
  formatWindow
} from "./availabilityTone";

const base: AppAvailabilityReading = {
  verdict: "healthy",
  objective: 0.99,
  windows: []
};

describe("availabilityTone", () => {
  it("renders a quiet app as muted, never as ok", () => {
    // The whole reason `no_opinion` is a distinct verdict: an app nobody has
    // used has not been shown to work, and a green tick would say it had.
    const tone = availabilityTone({ ...base, verdict: "no_opinion" });
    expect(tone).toBe("muted");
    expect(tone).not.toBe("ok");
    expect(availabilityLabel({ ...base, verdict: "no_opinion" })).toBe("no data");
  });

  it("separates the grade that pages from the one that does not", () => {
    const paging: AppAvailabilityReading = { ...base, verdict: "burning", severity: "page" };
    const ticket: AppAvailabilityReading = { ...base, verdict: "burning", severity: "ticket" };
    expect(availabilityTone(paging)).toBe("danger");
    expect(availabilityTone(ticket)).toBe("warn");
    expect(availabilityTone(paging)).not.toBe(availabilityTone(ticket));
  });

  it("renders a healthy app as ok", () => {
    expect(availabilityTone(base)).toBe("ok");
    expect(availabilityLabel(base)).toBe("serving");
  });
});

describe("formatWindow", () => {
  it("keeps sub-hour windows in minutes and whole hours in hours", () => {
    expect(formatWindow(5)).toBe("5m");
    expect(formatWindow(30)).toBe("30m");
    expect(formatWindow(60)).toBe("1h");
    expect(formatWindow(1440)).toBe("24h");
  });

  it("leaves a ragged window in minutes rather than printing a fraction", () => {
    expect(formatWindow(90)).toBe("90m");
  });
});

describe("formatObjective", () => {
  it("drops the decimal when the objective is a whole percent", () => {
    expect(formatObjective(0.99)).toBe("99%");
    expect(formatObjective(0.995)).toBe("99.5%");
  });
});
