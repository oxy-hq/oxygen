import { describe, expect, it } from "vitest";
import type { KioskDeviceRow } from "@/types/frontline";
import {
  awaitingTablet,
  DEFAULT_IDLE_TIMEOUT_SECONDS,
  idleTimeoutFromMinutes,
  idleTimeoutLabel,
  idleTimeoutMinutesField,
  idleTimeoutPatch,
  kioskState
} from "./frontline";

const NOW = Date.parse("2026-09-07T12:00:00Z");
const HOUR = 60 * 60 * 1000;

const kiosk = (over: Partial<KioskDeviceRow> = {}): KioskDeviceRow => ({
  id: "k1",
  name: "Front counter",
  return_to: null,
  created_at: new Date(NOW - HOUR).toISOString(),
  bound_at: null,
  last_seen_at: null,
  revoked_at: null,
  enrol_expires_at: new Date(NOW + 23 * HOUR).toISOString(),
  location_id: null,
  location_name: null,
  idle_timeout_seconds: DEFAULT_IDLE_TIMEOUT_SECONDS,
  ...over
});

describe("kioskState", () => {
  it("is waiting while the enrol link is live and no tablet has used it", () => {
    expect(kioskState(kiosk(), NOW)).toBe("waiting");
  });

  it("is bound once a tablet holds the cookie, even after the link would have expired", () => {
    const bound = kiosk({
      bound_at: new Date(NOW - HOUR).toISOString(),
      enrol_expires_at: new Date(NOW - 10 * HOUR).toISOString()
    });
    expect(kioskState(bound, NOW)).toBe("bound");
  });

  it("is expired when the link lapsed unused", () => {
    expect(kioskState(kiosk({ enrol_expires_at: new Date(NOW - 1).toISOString() }), NOW)).toBe(
      "expired"
    );
    expect(kioskState(kiosk({ enrol_expires_at: null }), NOW)).toBe("expired");
  });

  it("is revoked whatever else happened — revoked beats bound", () => {
    const revoked = kiosk({
      bound_at: new Date(NOW - HOUR).toISOString(),
      revoked_at: new Date(NOW).toISOString()
    });
    expect(kioskState(revoked, NOW)).toBe("revoked");
  });
});

describe("awaitingTablet", () => {
  it("is true only while some kiosk is still waiting", () => {
    const bound = kiosk({ id: "k2", bound_at: new Date(NOW).toISOString() });
    const revoked = kiosk({ id: "k3", revoked_at: new Date(NOW).toISOString() });
    const expired = kiosk({ id: "k4", enrol_expires_at: new Date(NOW - 1).toISOString() });
    expect(awaitingTablet([bound, revoked, expired], NOW)).toBe(false);
    expect(awaitingTablet([bound, kiosk()], NOW)).toBe(true);
    expect(awaitingTablet([], NOW)).toBe(false);
  });
});

describe("idleTimeoutLabel", () => {
  it("marks the platform default, because the wire cannot", () => {
    // Every `idle_timeout_seconds` the server sends is already effective — a
    // kiosk that named no number and one that asked for exactly the default
    // arrive identical — so the list says which tablets are simply following
    // the platform.
    expect(DEFAULT_IDLE_TIMEOUT_SECONDS).toBe(1800);
    expect(idleTimeoutLabel(DEFAULT_IDLE_TIMEOUT_SECONDS)).toBe("30 minutes (default)");
  });

  it("spells the values either side of it", () => {
    expect(idleTimeoutLabel(60)).toBe("1 minute");
    expect(idleTimeoutLabel(45)).toBe("45 seconds");
    expect(idleTimeoutLabel(90)).toBe("1 minute 30 seconds");
    expect(idleTimeoutLabel(3600)).toBe("1 hour");
    expect(idleTimeoutLabel(12 * 60 * 60)).toBe("12 hours");
    expect(idleTimeoutLabel(0)).toBe("—");
  });
});

describe("idleTimeoutFromMinutes", () => {
  it("sends nothing for an empty box, so the kiosk follows the default later too", () => {
    expect(idleTimeoutFromMinutes("")).toEqual({ kind: "default" });
    expect(idleTimeoutFromMinutes("   ")).toEqual({ kind: "default" });
  });

  it("converts whole minutes inside the server's window", () => {
    expect(idleTimeoutFromMinutes("1")).toEqual({ kind: "seconds", seconds: 60 });
    expect(idleTimeoutFromMinutes("30")).toEqual({ kind: "seconds", seconds: 1800 });
    expect(idleTimeoutFromMinutes("720")).toEqual({ kind: "seconds", seconds: 43200 });
  });

  it("refuses what the server would 400 on, and says so in minutes", () => {
    // 0 is ambiguous ("instantly" or "never"), 721 minutes outlives the shift
    // session itself, and a fraction is not a number of minutes.
    for (const bad of ["0", "-5", "721", "1.5", "half an hour"]) {
      const answer = idleTimeoutFromMinutes(bad);
      expect(answer.kind).toBe("invalid");
    }
    const refusal = idleTimeoutFromMinutes("0");
    expect(refusal.kind === "invalid" && refusal.message).toContain("30 minutes (default)");
  });
});

describe("idleTimeoutMinutesField", () => {
  it("is empty for a kiosk on the default, exactly as the enrol dialog's box is", () => {
    // The wire cannot tell a NULL column from an explicit 1800 — the server
    // resolves it before serializing — so an empty box is the only honest
    // prefill, and it is the same empty box `New kiosk` shows for the default.
    expect(idleTimeoutMinutesField(DEFAULT_IDLE_TIMEOUT_SECONDS)).toBe("");
  });

  it("shows the minutes of a kiosk that named its own number", () => {
    expect(idleTimeoutMinutesField(60)).toBe("1");
    expect(idleTimeoutMinutesField(900)).toBe("15");
    expect(idleTimeoutMinutesField(12 * 60 * 60)).toBe("720");
  });

  it("rounds a sub-minute kiosk to the minute the box can say", () => {
    // 30–59 s is reachable through the API, not through this box. Showing "1"
    // is not a silent change: the field is labelled minutes, and nothing is
    // written unless the admin saves it.
    expect(idleTimeoutMinutesField(45)).toBe("1");
    expect(idleTimeoutMinutesField(30)).toBe("1");
  });
});

describe("idleTimeoutPatch", () => {
  it("clears with null, which is what puts a kiosk back on the default", () => {
    // Not `{}` and not 1800: absent would leave the row alone, and the number
    // would freeze today's default into it. `null` is the third state, and the
    // only one that follows the platform if the default moves again.
    expect(idleTimeoutPatch(idleTimeoutFromMinutes(""))).toEqual({
      idle_timeout_seconds: null
    });
  });

  it("sends the seconds the box asked for", () => {
    expect(idleTimeoutPatch(idleTimeoutFromMinutes("15"))).toEqual({
      idle_timeout_seconds: 900
    });
  });

  it("sends nothing at all for a box the server would refuse", () => {
    expect(idleTimeoutPatch(idleTimeoutFromMinutes("0"))).toBeNull();
    expect(idleTimeoutPatch(idleTimeoutFromMinutes("721"))).toBeNull();
  });
});
