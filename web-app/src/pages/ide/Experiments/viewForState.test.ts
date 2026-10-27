import { describe, expect, it } from "vitest";
import type { ExperimentStatus } from "@/types/experiment";
import { badgeVariantFor, mayFetchResult, statusLabel, viewForState } from "./viewForState";

const STATUSES: ExperimentStatus[] = [
  "draft",
  "registrable",
  "registration_refused",
  "registered",
  "live",
  "closing",
  "reported",
  "aborted"
];

describe("viewForState", () => {
  it.each([
    ["draft", "pricing"],
    ["registrable", "registrable"],
    ["registration_refused", "refused"],
    ["registered", "operations"],
    ["live", "operations"],
    ["closing", "operations"],
    ["reported", "report"],
    ["aborted", "report"]
  ] as const)("%s shows the %s view", (status, view) => {
    expect(viewForState(status)).toBe(view);
  });

  it("maps a status this build does not know to the unknown view, never to a result", () => {
    expect(viewForState("archived")).toBe("unknown");
    expect(mayFetchResult("archived")).toBe(false);
  });
});

describe("mayFetchResult", () => {
  it("is true only once the run has reported or aborted", () => {
    expect(STATUSES.filter((s) => mayFetchResult(s))).toEqual(["reported", "aborted"]);
  });
});

describe("statusLabel / badgeVariantFor", () => {
  it("labels every status in plain words", () => {
    expect(STATUSES.map(statusLabel)).toEqual([
      "Draft",
      "Registrable",
      "Registration refused",
      "Registered",
      "Live",
      "Closing",
      "Reported",
      "Aborted"
    ]);
  });

  it("falls back to the raw string for an unknown status", () => {
    expect(statusLabel("archived")).toBe("archived");
    expect(badgeVariantFor("archived")).toBe("outline");
  });

  it("marks the two failure statuses destructive", () => {
    expect(badgeVariantFor("aborted")).toBe("destructive");
    expect(badgeVariantFor("registration_refused")).toBe("destructive");
  });
});
