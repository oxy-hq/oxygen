import { describe, expect, it } from "vitest";
import type { AppFunctionSummary } from "@/types/apps";
import {
  formatInZone,
  isDue,
  lastScheduledRun,
  NEXT_RUN_REFRESH_CEILING_MS,
  NEXT_RUN_REFRESH_FLOOR_MS,
  nextRunRefreshMs,
  scheduledFunctions,
  scheduleSwitch
} from "./schedules";

const fn = (over: Partial<AppFunctionSummary>): AppFunctionSummary => ({
  name: "fn",
  route: true,
  schedule: null,
  timezone: null,
  airway: false,
  timeout_seconds: null,
  retries: null,
  secrets_write: false,
  destinations: [],
  ...over
});

describe("scheduledFunctions", () => {
  it("keeps the scheduled ones, soonest first, and gives a bare schedule UTC", () => {
    const rows = scheduledFunctions([
      fn({ name: "nightly", schedule: "0 2 * * *", next_run_at: "2026-11-18T02:00:00Z" }),
      fn({ name: "echo" }),
      fn({
        name: "hourly",
        schedule: "0 * * * *",
        timezone: "Europe/Oslo",
        next_run_at: "2026-11-17T16:00:00Z"
      }),
      fn({ name: "broken", schedule: "every tuesday", next_run_at: null })
    ]);
    expect(rows.map((r) => r.name)).toEqual(["hourly", "nightly", "broken"]);
    expect(rows[0]).toMatchObject({ timezone: "Europe/Oslo", timezoneDeclared: true });
    expect(rows[1]).toMatchObject({ timezone: "UTC", timezoneDeclared: false });
    // The scheduler cannot read it, so there is no next run — not a made-up one.
    expect(rows[2].nextRunAt).toBeNull();
  });
});

describe("scheduleSwitch", () => {
  it("is always on in production, which has no switch", () => {
    expect(scheduleSwitch("production", { pending: true, failed: false })).toEqual({
      state: "always"
    });
  });

  it("reads a non-production environment's switch", () => {
    const read = { pending: false, failed: false };
    expect(scheduleSwitch("staging", { ...read, enabled: true })).toEqual({ state: "on" });
    expect(scheduleSwitch("dev-a1", { ...read, enabled: false })).toEqual({ state: "off" });
  });

  it("does not guess when it could not read the switch", () => {
    expect(scheduleSwitch("staging", { pending: true, failed: false })).toEqual({
      state: "unknown",
      why: "loading"
    });
    expect(scheduleSwitch("staging", { pending: false, failed: true, status: 403 })).toEqual({
      state: "unknown",
      why: "forbidden"
    });
    expect(scheduleSwitch("staging", { pending: false, failed: true, status: 404 })).toEqual({
      state: "unknown",
      why: "missing"
    });
    expect(scheduleSwitch("staging", { pending: false, failed: true, status: 500 })).toEqual({
      state: "unknown",
      why: "error"
    });
  });
});

describe("lastScheduledRun", () => {
  it("is the newest cron fire, not a Run now", () => {
    const rows = [
      { id: "a", mode: "schedule", created_at: "2026-11-17T09:00:00Z" },
      { id: "b", mode: "manual", created_at: "2026-11-17T12:00:00Z" },
      { id: "c", mode: "schedule", created_at: "2026-11-17T10:00:00Z" },
      { id: "d", mode: "route", created_at: "2026-11-17T11:00:00Z" }
    ];
    expect(lastScheduledRun(rows)?.id).toBe("c");
    expect(lastScheduledRun(rows.filter((r) => r.mode !== "schedule"))).toBeNull();
  });
});

describe("formatInZone", () => {
  it("reads the instant on the cron's own clock", () => {
    const at = "2026-11-17T14:00:00Z";
    expect(formatInZone(at, "America/New_York")).toContain("09:00");
    expect(formatInZone(at, "UTC")).toContain("14:00");
    expect(formatInZone(at, "UTC")).toContain("17 Nov");
  });

  it("does not throw on an unknown zone or an unreadable instant", () => {
    expect(formatInZone("2026-11-17T14:00:00Z", "Mars/Olympus")).toBe("2026-11-17T14:00:00.000Z");
    expect(formatInZone("not a date", "UTC")).toBe("—");
  });
});

describe("a next run that has passed", () => {
  const now = Date.parse("2026-11-17T09:18:00Z");

  it("is due once its instant has gone by, and not before", () => {
    expect(isDue("2026-11-17T09:15:00Z", now)).toBe(true);
    expect(isDue("2026-11-17T09:18:00Z", now)).toBe(true);
    expect(isDue("2026-11-17T09:30:00Z", now)).toBe(false);
    expect(isDue(null, now)).toBe(false);
    expect(isDue("not a date", now)).toBe(false);
  });

  it("is read again just after the soonest next run, never sooner than the floor", () => {
    // Twelve minutes out: just after it, past the grace for the server's clock.
    expect(nextRunRefreshMs(["2026-11-17T10:00:00Z", "2026-11-17T09:30:00Z"], now)).toBe(
      12 * 60_000 + 5_000
    );
    // Already past, or about to be: the floor, so a skewed clock cannot spin it.
    expect(nextRunRefreshMs(["2026-11-17T09:15:00Z"], now)).toBe(NEXT_RUN_REFRESH_FLOOR_MS);
    expect(nextRunRefreshMs(["2026-11-17T09:18:02Z"], now)).toBe(NEXT_RUN_REFRESH_FLOOR_MS);
  });

  it("waits at most the ceiling, and not at all with nothing to wait for", () => {
    // A monthly cron is weeks out — past what setTimeout can hold.
    expect(nextRunRefreshMs(["2026-12-31T00:00:00Z"], now)).toBe(NEXT_RUN_REFRESH_CEILING_MS);
    expect(nextRunRefreshMs([null, ""], now)).toBeNull();
    expect(nextRunRefreshMs([], now)).toBeNull();
  });
});
