import { describe, expect, it } from "vitest";
import {
  durationText,
  isExtendable,
  isRegenerable,
  isRevocable,
  lastUsedText,
  tokenLifecycle
} from "./tokens";

const NOW = new Date("2026-10-01T12:00:00Z");
const inDays = (n: number) => new Date(NOW.getTime() + n * 24 * 60 * 60 * 1000).toISOString();

describe("durationText", () => {
  it("uses the largest whole unit", () => {
    const hour = 60 * 60 * 1000;
    expect(durationText(36 * hour)).toBe("1 day");
    expect(durationText(72 * hour)).toBe("3 days");
    expect(durationText(5 * hour)).toBe("5 hours");
    expect(durationText(hour)).toBe("1 hour");
    expect(durationText(10 * 60 * 1000)).toBe("10 minutes");
  });

  it("never says zero minutes", () => {
    expect(durationText(5_000)).toBe("1 minute");
  });
});

describe("tokenLifecycle", () => {
  it("is active with a countdown well before expiry", () => {
    expect(
      tokenLifecycle({ status: "active", expires_at: inDays(30), revoked_at: null }, NOW)
    ).toEqual({ tone: "active", label: "Active", detail: "Expires in 30 days" });
  });

  it("flags a token inside its last week", () => {
    const life = tokenLifecycle({ status: "active", expires_at: inDays(3), revoked_at: null }, NOW);
    expect(life.tone).toBe("soon");
    expect(life.label).toBe("Active");
  });

  it("says a token never expires rather than leaving the cell empty", () => {
    expect(tokenLifecycle({ status: "active", expires_at: null, revoked_at: null }, NOW)).toEqual({
      tone: "active",
      label: "Active",
      detail: "No expiry"
    });
  });

  it("is expired when the server says so", () => {
    const life = tokenLifecycle(
      { status: "expired", expires_at: inDays(-2), revoked_at: null },
      NOW
    );
    expect(life.tone).toBe("expired");
    expect(life.detail).toMatch(/^Expired /);
  });

  it("is expired once the clock passes expires_at, even if the row still says active", () => {
    const life = tokenLifecycle(
      { status: "active", expires_at: inDays(-1), revoked_at: null },
      NOW
    );
    expect(life.tone).toBe("expired");
  });

  it("is revoked ahead of anything else", () => {
    const life = tokenLifecycle(
      { status: "revoked", expires_at: inDays(30), revoked_at: inDays(-1) },
      NOW
    );
    expect(life.tone).toBe("revoked");
    expect(life.detail).toMatch(/^Revoked /);
  });
});

describe("row actions", () => {
  const token = (over: object = {}) => ({
    kind: "service_account" as const,
    status: "active" as const,
    expires_at: inDays(30) as string | null,
    ...over
  });

  it("offers Extend on a live token with an expiry", () => {
    expect(isExtendable(token())).toBe(true);
  });

  it("offers Extend on an expired token: that is how it comes back", () => {
    expect(isExtendable(token({ status: "expired", expires_at: inDays(-3) }))).toBe(true);
  });

  it("offers no Extend on a token that never expires", () => {
    expect(isExtendable(token({ expires_at: null }))).toBe(false);
  });

  it("offers no Extend on a revoked token", () => {
    expect(isExtendable(token({ status: "revoked" }))).toBe(false);
  });

  it("offers no Extend on a trusted-access token", () => {
    expect(isExtendable(token({ kind: "ci" }))).toBe(false);
  });

  it("regenerates only a live service-account token", () => {
    expect(isRegenerable(token())).toBe(true);
    expect(isRegenerable(token({ status: "expired" }))).toBe(true);
    expect(isRegenerable(token({ status: "revoked" }))).toBe(false);
    expect(isRegenerable(token({ kind: "legacy_key" }))).toBe(false);
  });

  it("revokes anything not already revoked", () => {
    expect(isRevocable(token({ status: "expired" }))).toBe(true);
    expect(isRevocable(token({ status: "revoked" }))).toBe(false);
  });
});

describe("lastUsedText", () => {
  it("says Never for a token that was never used", () => {
    expect(lastUsedText(null, NOW)).toBe("Never");
  });

  it("counts recent days in words", () => {
    expect(lastUsedText(inDays(0), NOW)).toBe("Today");
    expect(lastUsedText(inDays(-1), NOW)).toBe("Yesterday");
    expect(lastUsedText(inDays(-5), NOW)).toBe("5 days ago");
  });

  it("switches to the date after a week", () => {
    expect(lastUsedText(inDays(-30), NOW)).toMatch(/2026$/);
  });
});
