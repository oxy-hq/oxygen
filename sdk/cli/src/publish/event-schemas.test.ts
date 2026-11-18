import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { OxyAppManifest } from "@oxy-hq/sdk/manifest";
import { afterAll, describe, expect, it } from "vitest";

import { embedEventSchemas } from "./event-schemas.js";

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
