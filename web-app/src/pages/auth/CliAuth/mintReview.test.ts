import { describe, expect, it } from "vitest";
import { DEFAULT_SANDBOX_LIMITS as LIMITS } from "@/libs/sandboxAgentToken";
import type { SandboxApp } from "@/types/apiToken";
import type { MintAsk } from "./cliAuthRequest";
import { reviewMintAsk } from "./mintReview";

const app = (id: string, org: string, slug: string, name: string): SandboxApp => ({
  id,
  org_id: `org-${org}`,
  org_slug: org,
  org_name: org.charAt(0).toUpperCase() + org.slice(1),
  slug,
  name
});

const APPS = [
  app("a1", "acme", "store-ops", "Store Ops"),
  app("a2", "acme", "refunds", "Refunds"),
  app("a3", "acme", "labor", "Labor"),
  app("a4", "globex", "pos", "POS"),
  app("a5", "globex", "inventory", "Inventory"),
  app("a6", "globex", "payroll", "Payroll")
];

const ask = (over: Partial<MintAsk> = {}): MintAsk => ({
  apps: ["acme/store-ops", "globex/pos"],
  hours: "8",
  name: "refunds task",
  ...over
});

const review = (over: Partial<MintAsk> = {}, apps: SandboxApp[] = APPS) =>
  reviewMintAsk(ask(over), "luong-mbp", apps, LIMITS);

const NONE = { apps: null, hours: null, name: null };

describe("reviewMintAsk", () => {
  it("resolves the references to app ids and builds the authorize body's mint", () => {
    const result = review();
    expect(result.problems).toEqual(NONE);
    expect(result.apps.map((line) => line.app?.name)).toEqual(["Store Ops", "POS"]);
    expect(result.mint).toEqual({
      kind: "sandbox_agent",
      apps: ["a1", "a4"],
      expires_in_hours: 8,
      name: "refunds task"
    });
  });

  it("refuses an app that doesn't resolve, marking its own line, and builds no body", () => {
    const one = review({ apps: ["acme/store-ops", "acme/ghost"] });
    expect(one.apps).toEqual([
      { ref: "acme/store-ops", app: APPS[0] },
      { ref: "acme/ghost", app: undefined }
    ]);
    expect(one.mint).toBeUndefined();
    // The line carries it: nothing is wrong with the apps as a whole, the hours or the name.
    expect(one.problems).toEqual(NONE);

    const three = review({ apps: ["acme/ghost", "store-ops", "globex/x"] });
    expect(three.apps.every((line) => line.app === undefined)).toBe(true);
    expect(three.mint).toBeUndefined();
  });

  it("refuses everything for someone who may mint for no app", () => {
    const result = review({}, []);
    expect(result.mint).toBeUndefined();
    expect(result.apps.map((line) => [line.ref, line.app])).toEqual([
      ["acme/store-ops", undefined],
      ["globex/pos", undefined]
    ]);
  });

  it("refuses a request with no app, or with more than the limit", () => {
    const none = review({ apps: [] });
    expect(none.problems.apps).toBe("A sandbox agent token covers at least one app.");
    expect(none.mint).toBeUndefined();
    const six = review({ apps: APPS.map((each) => `${each.org_slug}/${each.slug}`) });
    expect(six.apps).toHaveLength(6);
    expect(six.problems.apps).toBe("A sandbox agent token covers at most 5 apps.");
    expect(six.mint).toBeUndefined();
  });

  it("counts an app once when oxyc spells it twice, since a repeat is a 400", () => {
    const result = review({
      apps: ["acme/store-ops", "ACME/Store-Ops", "acme/ghost", "ACME/GHOST"]
    });
    expect(result.apps.map((line) => line.ref)).toEqual(["acme/store-ops", "acme/ghost"]);
    expect(review({ apps: ["acme/store-ops", "ACME/Store-Ops"] }).mint?.apps).toEqual(["a1"]);
  });

  it("uses the server's default lifetime when oxyc sent none", () => {
    expect(review({ hours: null }).mint?.expires_in_hours).toBe(8);
    expect(
      reviewMintAsk(ask({ hours: null }), "h", APPS, {
        default_hours: 4,
        max_hours: 48,
        max_apps: 5
      }).mint?.expires_in_hours
    ).toBe(4);
  });

  it("refuses a lifetime out of range or not a number, keeping what was asked", () => {
    const long = review({ hours: "500" });
    expect(long.hours).toBe(500);
    expect(long.problems.hours).toBe("A sandbox agent token lasts at most 168 hours (7 days).");
    expect(long.mint).toBeUndefined();

    expect(review({ hours: "0" }).problems.hours).toBe(
      "A sandbox agent token lasts at least 1 hour."
    );
    const words = review({ hours: "soon" });
    expect(words.hours).toBeNull();
    expect(words.problems.hours).toBe("Not a whole number of hours.");
    expect(words.mint).toBeUndefined();
    expect(review({ hours: "1.5" }).mint).toBeUndefined();
    expect(review({ hours: "168" }).problems.hours).toBeNull();
    expect(review({ hours: "168" }).mint?.expires_in_hours).toBe(168);
  });

  it("names the token after the computer when oxyc sent no name, and refuses one too long", () => {
    expect(review({ name: null }).mint?.name).toBe("Sandbox agent on luong-mbp");
    expect(review({ name: "   " }).mint?.name).toBe("Sandbox agent on luong-mbp");
    expect(review({ name: "x".repeat(100) }).problems).toEqual(NONE);
    const long = review({ name: "x".repeat(101) });
    expect(long.problems.name).toBe("The token's name is longer than 100 characters.");
    expect(long.mint).toBeUndefined();
  });

  it("marks every problem at once, so one run of oxyc can fix them all", () => {
    const result = review({ apps: ["acme/ghost"], hours: "500", name: "x".repeat(101) });
    expect(result.apps[0].app).toBeUndefined();
    expect(result.problems.hours).not.toBeNull();
    expect(result.problems.name).not.toBeNull();
    expect(result.mint).toBeUndefined();
  });
});
