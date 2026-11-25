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

  // The relay is the platform moving an org's events; it is nobody's job.
  it("treats the app events relay as a system daemon", () => {
    expect(isSystemSource("app_events_relay")).toBe(true);
  });

  // The table tracker reads a watched table's changes and publishes events.
  it("treats the table tracker as a system daemon", () => {
    expect(isSystemSource("app_table_watch")).toBe(true);
  });

  // The change stream relay ships an org's captured OLTP changes to Airhouse.
  it("treats the change stream relay as a system daemon", () => {
    expect(isSystemSource("oltp_changes_relay")).toBe(true);
  });

  it("files a chunked backfill's drive under ELT with the runs it seeds", () => {
    expect(sourceTypeToJobType("airway_backfill_range")).toBe("elt");
    expect(isSystemSource("airway_backfill_range")).toBe(false);
  });

  it("falls back to an agent run for a source it does not know", () => {
    expect(sourceTypeToJobType("something_new")).toBe("agent");
    expect(sourceTypeToJobType(null)).toBe("agent");
  });
});
