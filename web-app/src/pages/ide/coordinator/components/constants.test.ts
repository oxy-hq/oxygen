import { describe, expect, it } from "vitest";
import { isSystemSource, sourceTypeToJobType } from "./constants";

describe("sourceTypeToJobType", () => {
  it("files a scan and the Slack post it queues under the same job type", () => {
    expect(sourceTypeToJobType("monitor_scan")).toBe("monitor");
    expect(sourceTypeToJobType("anomaly_notify")).toBe("monitor");
  });

  // Insights delivery exists because the workspace's own file asked for it, so
  // it keeps its job-type badge instead of the one for platform daemons.
  it("does not treat insights delivery as a system daemon", () => {
    expect(isSystemSource("anomaly_notify")).toBe(false);
    expect(isSystemSource("preagg_cycle")).toBe(true);
  });

  it("falls back to an agent run for a source it does not know", () => {
    expect(sourceTypeToJobType("something_new")).toBe("agent");
    expect(sourceTypeToJobType(null)).toBe("agent");
  });
});
