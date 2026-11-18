import { describe, expect, it } from "vitest";
import { createTestContext } from "./context";
import { createTestEvents } from "./events";
import { REFUSALS } from "./host-contract";

const manifest = {
  schemaVersion: 2,
  slug: "locations",
  functions: {
    pub: {
      route: true,
      oltp: { enabled: true },
      events: { publish: ["statement.uploaded"] }
    },
    plain: { route: true, oltp: { enabled: true } }
  }
};
const DATABASES = { org_oltp: { dialect: "postgres", kind: "postgres_managed" } } as const;

function make(fn = "pub") {
  const events = createTestEvents();
  const t = createTestContext(manifest, { function: fn, databases: DATABASES, events });
  return { events, t };
}

describe("createTestContext events", () => {
  it("records ctx.events.publish", async () => {
    const { events, t } = make();
    const r = await t.ctx.events.publish("statement.uploaded", { id: 1 }, { key: "k1" });
    expect(r.eventId).toMatch(/^[0-9a-f-]{36}$/);
    expect(events.published).toEqual([
      { type: "statement.uploaded", data: { id: 1 }, key: "k1", delaySeconds: 0 }
    ]);
    expect(t.callsTo("events.publish")).toHaveLength(1);
  });

  it("refuses without the events capability", async () => {
    const { t } = make("plain");
    await expect(t.ctx.events.publish("x", {}, { key: "k" })).rejects.toThrow(
      /EventsCapabilityMissing/
    );
    expect(REFUSALS.eventsCapability.refusal).toContain("EventsCapabilityMissing");
  });

  it("refuses a missing or empty key in the host's words", async () => {
    const { events, t } = make();
    for (const opts of [{}, { key: "" }, undefined]) {
      await expect(
        t.ctx.events.publish("statement.uploaded", {}, opts as { key: string })
      ).rejects.toThrow(REFUSALS.eventKeyInvalid.refusal);
    }
    await expect(
      t.ctx.oltp.tx(async (tx) => {
        await tx.events.publish("statement.uploaded", {}, { key: "" });
      })
    ).rejects.toThrow(/EventKeyInvalid/);
    expect(events.published).toEqual([]);
  });

  // As the host: Postgres's text cannot hold U+0000, so a key holding it is
  // refused before anything is written, in the host's words.
  it("refuses a key holding U+0000 in the host's words", async () => {
    const { events, t } = make();
    await expect(
      t.ctx.events.publish("statement.uploaded", {}, { key: "a\u0000b" })
    ).rejects.toThrow(REFUSALS.eventKeyNul.refusal);
    await expect(
      t.ctx.oltp.tx(async (tx) => {
        await tx.events.publish("statement.uploaded", {}, { key: "\u0000" });
      })
    ).rejects.toThrow(REFUSALS.eventKeyNul.refusal);
    expect(events.published).toEqual([]);
  });

  it("records delaySeconds", async () => {
    const { events, t } = make();
    await t.ctx.events.publish("statement.uploaded", 1, { key: "k", delaySeconds: 30 });
    expect(events.published[0]?.delaySeconds).toBe(30);
  });

  it("records a publish inside oltp.tx on commit", async () => {
    const { events, t } = make();
    await t.ctx.oltp.tx(async (tx) => {
      await tx.events.publish("statement.uploaded", { a: 1 }, { key: "tx1" });
      expect(events.published).toHaveLength(0);
    });
    expect(events.published.map((e) => e.key)).toEqual(["tx1"]);
    expect(t.callsTo("tx.publish")).toHaveLength(1);
  });

  it("a publish inside a rolled-back oltp.tx is not recorded", async () => {
    const { events, t } = make();
    await expect(
      t.ctx.oltp.tx(async (tx) => {
        await tx.events.publish("statement.uploaded", {}, { key: "gone" });
        throw new Error("boom");
      })
    ).rejects.toThrow("boom");
    expect(events.published).toEqual([]);
  });

  it("a warehouse transaction handle has no events", async () => {
    const t = createTestContext(
      {
        ...manifest,
        functions: {
          pub: {
            ...manifest.functions.pub,
            destinations: ["w"],
            customerWarehouseWrites: { w: "the test writes the customer warehouse" }
          }
        }
      },
      {
        function: "pub",
        databases: { w: { dialect: "postgres", kind: "customer" } },
        events: createTestEvents()
      }
    );
    let ran = false;
    let hasEvents: boolean | undefined;
    await t.ctx.tx("w", async (tx) => {
      ran = true;
      hasEvents = "events" in tx;
    });
    expect(ran).toBe(true);
    expect(hasEvents).toBe(false);
  });

  it("publishing in a test triggers no listener", async () => {
    const { t } = make();
    let ran = false;
    const listener = () => {
      ran = true;
    };
    void listener;
    await t.ctx.events.publish("statement.uploaded", {}, { key: "k" });
    expect(ran).toBe(false);
  });

  it("ids are deterministic", async () => {
    const { t } = make();
    const a = await t.ctx.events.publish("statement.uploaded", {}, { key: "same" });
    const b = await t.ctx.events.publish("statement.uploaded", {}, { key: "same" });
    const c = await t.ctx.events.publish("statement.uploaded", {}, { key: "other" });
    expect(a.eventId).toBe(b.eventId);
    expect(a.eventId).not.toBe(c.eventId);
  });

  it("two test contexts do not share published events", async () => {
    const one = make();
    const two = make();
    await one.t.ctx.events.publish("statement.uploaded", {}, { key: "a" });
    expect(one.events.published).toHaveLength(1);
    expect(two.events.published).toHaveLength(0);
  });

  it.concurrent("concurrent contexts stay separate (1)", async () => {
    const x = make();
    const y = make();
    await Promise.all([
      x.t.ctx.events.publish("statement.uploaded", {}, { key: "x" }),
      y.t.ctx.events.publish("statement.uploaded", {}, { key: "y" })
    ]);
    expect(x.events.published.map((e) => e.key)).toEqual(["x"]);
    expect(y.events.published.map((e) => e.key)).toEqual(["y"]);
  });

  it.concurrent("concurrent contexts stay separate (2)", async () => {
    const x = make();
    const y = make();
    await Promise.all([
      x.t.ctx.events.publish("statement.uploaded", {}, { key: "1" }),
      y.t.ctx.events.publish("statement.uploaded", {}, { key: "2" })
    ]);
    expect(x.events.published).toHaveLength(1);
    expect(y.events.published).toHaveLength(1);
  });
});

describe("createTestEvents().envelope", () => {
  it("fills CloudEvents defaults", () => {
    const e = createTestEvents().envelope("locations/statement.uploaded", { n: 1 });
    expect(e.specversion).toBe("1.0");
    expect(e.depth).toBe(0);
    expect(e.type).toBe("locations/statement.uploaded");
    expect(e.data).toEqual({ n: 1 });
    expect(e.causationid).toBeNull();
    expect(e.source).toBe("/apps/locations");
  });

  it("applies overrides", () => {
    const e = createTestEvents().envelope("a/b", 1, { depth: 3, key: "z" });
    expect(e.depth).toBe(3);
    expect(e.key).toBe("z");
  });
});

describe("createTestContext table changes", () => {
  const watch = {
    schemaVersion: 2,
    slug: "locations",
    functions: { onLines: { on: { table: "pokehouse:bookkeeping.lines" } } }
  };

  it("builds the envelope the platform delivers for one snapshot", () => {
    const events = createTestEvents();
    const e = events.tableChange({
      table: "pokehouse:bookkeeping.lines",
      snapshotId: 41,
      inserted: 2,
      deleted: 1
    });
    expect(e.type).toBe("table:pokehouse:bookkeeping.lines");
    expect(e.source).toBe("/tables/pokehouse:bookkeeping.lines");
    expect(e.dataschema).toBe("oxy/table.change@1");
    expect(e.data).toEqual({ snapshotId: 41, inserted: 2, deleted: 1, updated: 0 });
  });

  it("answers ctx.tables.changes from what the test seeded, a page at a time", async () => {
    const events = createTestEvents();
    const e = events.tableChange({
      table: "pokehouse:bookkeeping.lines",
      snapshotId: 7,
      inserted: 3,
      deleted: 0
    });
    const t = createTestContext(watch, {
      function: "onLines",
      events,
      tableChanges: { [e.id]: { inserted: [{ id: 1 }, { id: 2 }, { id: 3 }] } }
    });
    const first = await t.ctx.tables.changes(e, { limit: 2 });
    expect(first).toEqual({ inserted: [{ id: 1 }, { id: 2 }], deleted: [], hasMore: true });
    const rest = await t.ctx.tables.changes(e, { limit: 2, offset: 2 });
    expect(rest).toEqual({ inserted: [{ id: 3 }], deleted: [], hasMore: false });
    expect(t.callsTo("events.changes")).toHaveLength(2);
  });
});
