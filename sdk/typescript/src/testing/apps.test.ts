import { describe, expect, it } from "vitest";
import { type AppCallError, isAppCallError } from "../apps";
import { createTestContext } from "./context";
import type { ManifestLike } from "./gates";
import { REFUSALS } from "./host-contract";

const manifest = {
  schemaVersion: 2,
  slug: "orders",
  calls: { locations: ["lookup"], billing: ["charge", "refund"] },
  functions: { place: { route: true } }
};
const bare = { schemaVersion: 2, slug: "orders", functions: { place: { route: true } } };

/** The error `p` rejects with; fails the test when it resolves. */
async function fail(p: Promise<unknown>): Promise<AppCallError> {
  try {
    await p;
  } catch (e) {
    return e as AppCallError;
  }
  throw new Error("expected the call to throw");
}

function make(
  apps?: Record<string, (b: unknown, o?: object) => unknown>,
  m: ManifestLike = manifest
) {
  return createTestContext(m, { function: "place", apps });
}

describe("createTestContext apps", () => {
  it("runs the registered handler and round-trips JSON", async () => {
    const seen: unknown[] = [];
    const t = make({
      "locations/lookup": (body, opts) => {
        seen.push([body, opts]);
        return { id: "l1", when: new Date(0), skip: undefined };
      }
    });
    const out = await t.ctx.apps.call("locations", "lookup", { code: "A" }, { timeoutMs: 5 });
    expect(out).toEqual({ id: "l1", when: "1970-01-01T00:00:00.000Z" });
    expect(seen).toEqual([[{ code: "A" }, { timeoutMs: 5 }]]);
    expect(t.callsTo("apps.call")).toHaveLength(1);
  });

  it("answers null for a handler that returns nothing", async () => {
    const t = make({ "locations/lookup": () => undefined });
    expect(await t.ctx.apps.call("locations", "lookup")).toBeNull();
  });

  it("throws not_found for a declared pair with no handler", async () => {
    const t = make({});
    const err = await fail(t.ctx.apps.call("billing", "charge"));
    expect(isAppCallError(err)).toBe(true);
    expect(err.kind).toBe("not_found");
    expect(err.hostMessage).toMatch(/^AppCallNotFound: /);
  });

  it("wraps a handler's throw as failed, cut to 500 characters", async () => {
    const t = make({
      "locations/lookup": () => {
        throw new Error("x".repeat(900));
      }
    });
    const err = await fail(t.ctx.apps.call("locations", "lookup"));
    expect(err.kind).toBe("failed");
    expect(err.message).toBe("x".repeat(500));
  });

  it("refuses an undeclared pair in the host's words", async () => {
    const t = make({ "locations/other": () => 1 });
    const err = await fail(t.ctx.apps.call("locations", "other"));
    expect(err.kind).toBe("refused");
    expect(err.hostMessage).toBe(
      'AppCallRefused: oxy-app.json does not declare a call to locations/other (add it under "calls")'
    );
    await expect(t.ctx.apps.send("locations", "other")).rejects.toMatchObject({ kind: "refused" });
    expect(t.state.apps.sent).toEqual([]);
  });

  it("refuses empty or non-string names", async () => {
    const t = make();
    for (const [a, f] of [
      ["", "lookup"],
      ["locations", ""],
      [1, "x"]
    ] as const) {
      const err = await fail(t.ctx.apps.call(a as string, f as string));
      expect(err.hostMessage).toBe(REFUSALS.appCallNames.refusal);
    }
  });

  it("refuses everything when the manifest declares no calls", async () => {
    const t = make({}, bare);
    await expect(t.ctx.apps.call("locations", "lookup")).rejects.toThrow(/AppsCapabilityMissing/);
    await expect(t.ctx.apps.send("locations", "lookup")).rejects.toThrow(/AppsCapabilityMissing/);
    expect(REFUSALS.appsCapability.refusal).toBe(
      'AppsCapabilityMissing: this function has not declared the apps it calls (add "calls": { "<app>": ["<function>"] } to oxy-app.json)'
    );
  });

  it("records sends, with a fake event id", async () => {
    const t = make();
    const a = await t.ctx.apps.send("billing", "charge", { n: 1 }, { key: "k", delaySeconds: 30 });
    const b = await t.ctx.apps.send("billing", "refund");
    expect(a.eventId).toMatch(/^[0-9a-f-]{36}$/);
    expect(a.eventId).not.toBe(b.eventId);
    expect(t.state.apps.sent).toEqual([
      {
        app: "billing",
        function: "charge",
        body: { n: 1 },
        key: "k",
        delaySeconds: 30,
        eventId: a.eventId
      },
      {
        app: "billing",
        function: "refund",
        body: null,
        key: null,
        delaySeconds: 0,
        eventId: b.eventId
      }
    ]);
    expect(t.callsTo("apps.send")).toHaveLength(2);
  });

  it("ctx.caller defaults to null and takes the option", () => {
    expect(make().ctx.caller).toBeNull();
    const caller = { app: "x", function: "y", invocationId: "i", actor: null };
    const t = createTestContext(manifest, { function: "place", caller });
    expect(t.ctx.caller).toEqual(caller);
  });
});
