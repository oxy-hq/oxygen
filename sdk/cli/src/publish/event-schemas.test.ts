import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { OxyAppManifest } from "@oxy-hq/sdk/manifest";
import { afterAll, describe, expect, it } from "vitest";

import { embedEventSchemas, embedEventSchemasIntoBundle } from "./event-schemas.js";

const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

function app(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-event-schemas-"));
  SCRATCH.push(dir);
  for (const [rel, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, rel)), { recursive: true });
    writeFileSync(join(dir, rel), content);
  }
  return dir;
}

const manifest = (schema: unknown) =>
  ({ events: { "statement.uploaded": { schema } } }) as unknown as OxyAppManifest;

describe("embedEventSchemas", () => {
  it("embeds a referenced schema", () => {
    const schema = { type: "object", properties: { id: { type: "string" } } };
    const dir = app({ "events/statement.json": JSON.stringify(schema) });
    const input = manifest("./events/statement.json");
    const out = embedEventSchemas(dir, input);
    expect(out.events?.["statement.uploaded"]?.schema).toEqual(schema);
    // The source manifest is not mutated.
    expect(input.events?.["statement.uploaded"]?.schema).toBe("./events/statement.json");
  });

  it("leaves an already-embedded schema alone", () => {
    const schema = { type: "object" };
    const out = embedEventSchemas(app({}), manifest(schema));
    expect(out.events?.["statement.uploaded"]?.schema).toEqual(schema);
  });

  it("fails naming the missing schema file", () => {
    const dir = app({});
    expect(() => embedEventSchemas(dir, manifest("./events/missing.json"))).toThrow(
      /statement\.uploaded.*events\/missing\.json/s
    );
  });

  it("fails naming the event and path when the file is not JSON", () => {
    const dir = app({ "bad.json": "{ nope" });
    expect(() => embedEventSchemas(dir, manifest("bad.json"))).toThrow(
      /statement\.uploaded.*bad\.json/s
    );
  });

  // As the server: an event's schema is at most 16 KiB serialized
  // (`FUNCTION_SCHEMA_MAX_BYTES`), whether written inline or in a file.
  it("refuses an event schema over the size cap, naming the event", () => {
    const wide = { type: "object", description: "x".repeat(16 * 1024) };
    const dir = app({ "events/wide.json": JSON.stringify(wide) });
    expect(() => embedEventSchemas(dir, manifest("./events/wide.json"))).toThrow(
      /statement\.uploaded.*over the 16384-byte cap/s
    );
    expect(() => embedEventSchemas(app({}), manifest(wide))).toThrow(
      /statement\.uploaded.*over the 16384-byte cap/s
    );
    const fits = { type: "object", description: "x".repeat(16 * 1024 - 64) };
    expect(
      embedEventSchemas(app({}), manifest(fits)).events?.["statement.uploaded"]?.schema
    ).toEqual(fits);
  });

  it("returns a manifest without events unchanged", () => {
    const m = { slug: "x" } as unknown as OxyAppManifest;
    expect(embedEventSchemas(app({}), m)).toEqual(m);
  });
});

describe("embedEventSchemasIntoBundle", () => {
  const v1 = { type: "object", properties: { id: { type: "string" } } };
  const v2 = { type: "object", properties: { id: { type: "integer" } } };
  const bundledManifest = (bundleDir: string): OxyAppManifest =>
    JSON.parse(readFileSync(join(bundleDir, "oxy-app.json"), "utf8")) as OxyAppManifest;

  it("embeds a bundle's schema paths without a source manifest", () => {
    const dir = app({
      "events/statement.json": JSON.stringify(v1),
      "out/oxy-app.json": JSON.stringify(manifest("./events/statement.json"))
    });
    const report = embedEventSchemasIntoBundle(dir, join(dir, "out"));
    expect(report).toEqual({ embedded: 1, refreshed: [] });
    expect(bundledManifest(join(dir, "out")).events?.["statement.uploaded"]?.schema).toEqual(v1);
  });

  // A `--dir` bundle published twice: the first publish rewrote the bundle's
  // manifest with the schema embedded, so only the source still names the file.
  it("re-embeds from the source's path over an embedded copy, naming what changed", () => {
    const dir = app({
      "events/statement.json": JSON.stringify(v2),
      "out/oxy-app.json": JSON.stringify(manifest(v1))
    });
    const report = embedEventSchemasIntoBundle(
      dir,
      join(dir, "out"),
      manifest("./events/statement.json")
    );
    expect(report).toEqual({ embedded: 1, refreshed: ["statement.uploaded"] });
    expect(bundledManifest(join(dir, "out")).events?.["statement.uploaded"]?.schema).toEqual(v2);
  });

  it("does not call an unchanged schema refreshed", () => {
    const dir = app({
      "events/statement.json": JSON.stringify(v1),
      "out/oxy-app.json": JSON.stringify(manifest(v1))
    });
    const report = embedEventSchemasIntoBundle(
      dir,
      join(dir, "out"),
      manifest("./events/statement.json")
    );
    expect(report).toEqual({ embedded: 1, refreshed: [] });
  });

  it("leaves an event the source writes inline, or does not declare, as the bundle has it", () => {
    const bundled = {
      events: { "statement.uploaded": { schema: v1 }, "statement.parsed": { schema: v1 } }
    };
    const dir = app({ "out/oxy-app.json": JSON.stringify(bundled) });
    const source = {
      events: { "statement.uploaded": { schema: v2 } }
    } as unknown as OxyAppManifest;
    const report = embedEventSchemasIntoBundle(dir, join(dir, "out"), source);
    expect(report).toEqual({ embedded: 0, refreshed: [] });
    expect(bundledManifest(join(dir, "out"))).toEqual(bundled);
  });
});
