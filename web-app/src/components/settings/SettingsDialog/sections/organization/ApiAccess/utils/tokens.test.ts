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

  // The four keys that were still in use on 2026-11-16 are deliberately left
  // unrevoked by the migration, so that reverting the image can restore them.
  // Their rows therefore read exactly like a live token's, and the only thing
  // that says otherwise is the status the server sends.
  it("says Removed for a legacy key, whatever the rest of the row says", () => {
    const removed = { tone: "removed", label: "Removed", detail: "Legacy API keys no longer authenticate" };
    // Not revoked, never expires: the live shape.
    expect(tokenLifecycle({ status: "removed", expires_at: null, revoked_at: null }, NOW)).toEqual(
      removed
    );
    // A future expiry, which would otherwise read "Expires in 30 days".
    expect(
      tokenLifecycle({ status: "removed", expires_at: inDays(30), revoked_at: null }, NOW)
    ).toEqual(removed);
    // Inside the last week, which would otherwise be the amber "soon" tone.
    expect(
      tokenLifecycle({ status: "removed", expires_at: inDays(3), revoked_at: null }, NOW)
    ).toEqual(removed);
    // And revoked as well: still Removed, not Revoked.
    expect(
      tokenLifecycle({ status: "removed", expires_at: inDays(-5), revoked_at: inDays(-1) }, NOW)
    ).toEqual(removed);
  });

  it("offers no action on a removed legacy key", () => {
    const key = { kind: "legacy_key", status: "removed", expires_at: inDays(30) } as const;
    expect(isExtendable(key)).toBe(false);
    expect(isRegenerable(key)).toBe(false);
    expect(isRevocable(key)).toBe(false);
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
