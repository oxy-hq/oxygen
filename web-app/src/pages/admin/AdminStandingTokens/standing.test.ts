import { describe, expect, it } from "vitest";
import { fromOxycLogin, madeWith, standingHint, standingLabel } from "./standing";

describe("standingLabel", () => {
  it("says staff, partner or both", () => {
    expect(standingLabel({ platform: true, partner: false })).toBe("Staff");
    expect(standingLabel({ platform: false, partner: true })).toBe("Partner");
    expect(standingLabel({ platform: true, partner: true })).toBe("Staff and partner");
  });

  it("does not invent a standing for a token carrying neither", () => {
    expect(standingLabel({ platform: false, partner: false })).toBe("None");
    expect(standingHint({ platform: false, partner: false })).toBeUndefined();
  });
});

describe("standingHint", () => {
  it("says what each standing lets the token do, in the third person", () => {
    expect(standingHint({ platform: true, partner: false })).toMatch(/its owner's Oxygen staff/);
    expect(standingHint({ platform: false, partner: true })).toMatch(/its owner's client org/);
  });

  it("says both for a token carrying both", () => {
    const hint = standingHint({ platform: true, partner: true });
    expect(hint).toMatch(/staff standing/);
    expect(hint).toMatch(/as a partner/);
  });
});

describe("madeWith", () => {
  it("reads `oxyc`, as this list sends it, as oxyc login", () => {
    expect(fromOxycLogin({ source: "oxyc" })).toBe(true);
    expect(madeWith({ source: "oxyc" }).label).toBe("oxyc login");
  });

  it("reads a personal token's own `oxyc_login` the same way", () => {
    expect(fromOxycLogin({ source: "oxyc_login" })).toBe(true);
  });

  it("reads `oxyc_agent` as an agent's token, which is not the login on its owner's machine", () => {
    const made = madeWith({ source: "oxyc_agent" });
    expect(made.label).toBe("oxyc agent");
    expect(made.hint).toMatch(/approved by its owner/);
    expect(fromOxycLogin({ source: "oxyc_agent" })).toBe(false);
  });

  it("reads `ui` as Settings, and not as oxyc login", () => {
    expect(fromOxycLogin({ source: "ui" })).toBe(false);
    expect(madeWith({ source: "ui" })).toEqual({
      label: "Settings",
      hint: "Made in Settings, under Personal access tokens."
    });
  });

  it("shows any other source as the server spelled it, with nothing claimed about it", () => {
    expect(madeWith({ source: "oidc" })).toEqual({ label: "oidc", hint: undefined });
    expect(madeWith({ source: "" }).label).toBe("Unknown");
  });
});
