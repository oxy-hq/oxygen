import { describe, expect, it } from "vitest";
import { announcedSeverity } from "./monitorNotify";

describe("announcedSeverity", () => {
  it("reads the floor as a floor", () => {
    const at = (min_severity: "low" | "medium" | "high") =>
      announcedSeverity({ slack_channel: "C0123ABCDEF", min_severity });
    expect(at("high")).toBe("at high severity");
    expect(at("medium")).toBe("at medium severity or above");
    expect(at("low")).toBe("of any severity");
  });
});
