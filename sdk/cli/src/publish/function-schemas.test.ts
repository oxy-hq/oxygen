/**
 * `oxyc publish` writing each function's `inputSchema` / `outputSchema` into
 * the bundle's `oxy-app.json`, against the fixture app in
 * `fixtures/function-schemas/`: one function per way a type is found, one the
 * generator cannot read, one whose manifest already says.
 */

import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkFunctionSchema,
  FUNCTION_SCHEMA_MAX_BYTES,
  serializedSchemaBytes
} from "@oxy-hq/sdk/manifest";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  appSourceReader,
  embedFunctionSchemas,
  functionTypesFromSource,
  GENERATED_COMMENT_PREFIX,
  shippedSchema
} from "./function-schemas.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const APP = resolve(HERE, "..", "..", "fixtures", "function-schemas");
const MANIFEST = JSON.parse(readFileSync(join(APP, "oxy-app.json"), "utf8"));

let bundle: string;

beforeEach(() => {
  // The build output: the plugin copies `oxy-app.json` into it.
  bundle = mkdtempSync(join(tmpdir(), "oxyc-fn-schemas-"));
  cpSync(join(APP, "oxy-app.json"), join(bundle, "oxy-app.json"));
});

afterEach(() => rmSync(bundle, { recursive: true, force: true }));

const bundled = () =>
  JSON.parse(readFileSync(join(bundle, "oxy-app.json"), "utf8")).functions as Record<
    string,
    Record<string, unknown>
  >;

describe("embedFunctionSchemas", () => {
  it("writes the schemas each function's types give, and says where each came from", () => {
    const report = embedFunctionSchemas(APP, bundle, MANIFEST);
    expect(report.generated).toEqual([
      "quote: inputSchema from `Input`",
      "quote: outputSchema from `Output`",
      "notify: inputSchema from the handler's `JSON.parse(req.body) as …`",
      "orders: outputSchema from the handler's return type",
      "sync: inputSchema from the handler's `JSON.parse(request.body) as …`",
      "nested-entry: inputSchema from `Input`"
    ]);

    const fns = bundled();
    expect(fns.quote?.inputSchema).toEqual({
      $schema: "https://json-schema.org/draft/2020-12/schema",
      $comment: `${GENERATED_COMMENT_PREFIX}functions/quote.ts (Input)`,
      description: "Ask for a delivery quote.",
      type: "object",
      properties: {
        store: { type: "integer", description: "The store to quote from.", minimum: 1 },
        kind: { type: "string", enum: ["pickup", "delivery"] },
        at: {
          type: "string",
          format: "date-time",
          description: "When the customer wants it, if not now."
        }
      },
      required: ["store", "kind"],
      additionalProperties: false
    });
    expect(fns.quote?.outputSchema).toMatchObject({
      properties: { price: { type: "number" }, currency: { enum: ["USD", "EUR"] } }
    });
    // A Response is not data: input only, and nothing said about the output.
    expect(fns.notify?.inputSchema).toMatchObject({
      properties: { name: { type: "string", description: "Who to greet." } }
    });
    expect(fns.notify?.outputSchema).toBeUndefined();
    // Through `import type { Order } from "./types"`.
    expect(fns.orders?.outputSchema).toMatchObject({
      properties: {
        orders: { type: "array", items: { properties: { totalCents: { type: "integer" } } } },
        total: { type: "number" }
      }
    });
    expect(fns.orders?.inputSchema).toBeUndefined();
    expect(fns.sync?.inputSchema).toMatchObject({ required: ["since"] });
    // A handler with no annotation says nothing about its output.
    expect(fns.sync?.outputSchema).toBeUndefined();
    expect(fns["nested-entry"]?.inputSchema).toMatchObject({
      $comment: `${GENERATED_COMMENT_PREFIX}functions/nested/entry.ts (Input)`,
      properties: { query: { type: "string", minLength: 1 } }
    });

    // Everything written passes the publish gate's check.
    for (const spec of Object.values(fns)) {
      for (const key of ["inputSchema", "outputSchema"]) {
        if (spec[key]) expect(checkFunctionSchema(spec[key]), key).toBeUndefined();
      }
    }
  });

  it("keeps a hand-written schema, and a hand-written null", () => {
    embedFunctionSchemas(APP, bundle, MANIFEST);
    expect(bundled().handwritten).toEqual(MANIFEST.functions.handwritten);
  });

  it("warns about a type it cannot read, naming the function, construct and line", () => {
    const report = embedFunctionSchemas(APP, bundle, MANIFEST);
    expect(report.warnings).toEqual([
      expect.stringMatching(
        /^function "tangled": no inputSchema from `Input` — intersection types .* \(functions\/tangled\.ts:6\)\. Write functions\.tangled\.inputSchema in oxy-app\.json by hand/
      )
    ]);
    expect(bundled().tangled?.inputSchema).toBeUndefined();
    // `Promise<void>` is not data either: no output, no warning.
    expect(bundled().tangled?.outputSchema).toBeUndefined();
  });

  it("regenerates what it wrote before, and drops it when the type is gone", () => {
    embedFunctionSchemas(APP, bundle, MANIFEST);
    const path = join(bundle, "oxy-app.json");
    const once = JSON.parse(readFileSync(path, "utf8"));
    once.functions.quote.inputSchema.description = "stale";
    once.functions.tangled.inputSchema = {
      $comment: `${GENERATED_COMMENT_PREFIX}functions/tangled.ts (Input)`,
      type: "object"
    };
    writeFileSync(path, JSON.stringify(once));

    embedFunctionSchemas(APP, bundle, MANIFEST);
    const twice = bundled();
    expect(twice.quote?.inputSchema).toMatchObject({ description: "Ask for a delivery quote." });
    expect(twice.tangled?.inputSchema).toBeUndefined();
  });

  it("leaves a bundle without a manifest, or one that is not JSON, alone", () => {
    rmSync(join(bundle, "oxy-app.json"));
    expect(embedFunctionSchemas(APP, bundle, MANIFEST)).toEqual({ generated: [], warnings: [] });
    writeFileSync(join(bundle, "oxy-app.json"), "{ not json");
    expect(embedFunctionSchemas(APP, bundle, MANIFEST)).toEqual({ generated: [], warnings: [] });
    expect(readFileSync(join(bundle, "oxy-app.json"), "utf8")).toBe("{ not json");
  });

  it("refuses an oversized result with a warning rather than shipping it", () => {
    const many = Array.from(
      { length: 600 },
      (_, i) => `  /** Field number ${i}. */\n  f${i}: string;`
    );
    const source = `export interface Input {\n${many.join("\n")}\n}\nexport default async function big() {}\n`;
    const files: Record<string, string> = { [join(APP, "functions", "quote.ts")]: source };
    const report = embedFunctionSchemas(APP, bundle, MANIFEST, (path) => files[path]);
    expect(
      report.warnings.some((w) => /"quote": the inputSchema .* over the 16384-byte cap/.test(w))
    ).toBe(true);
    expect(bundled().quote?.inputSchema).toBeUndefined();
  });

  // Publish measures the schema as it ships, `$comment` included. One that fits
  // the cap only without its comment must be a warning here, never a 422 there.
  it("measures the schema with its comment, as publish will", () => {
    const entry = join(APP, "functions", "quote.ts");
    const source = (n: number) =>
      `export interface Input {\n  /** ${"x".repeat(n)} */\n  a: string;\n}\n` +
      "export default async function quote() {}\n";
    const reader = (n: number) => (path: string) => (path === entry ? source(n) : undefined);
    const generated = (n: number) => {
      const input = functionTypesFromSource(entry, reader(n), APP).input;
      if (!input) throw new Error("no input generated");
      return input;
    };
    const bare = (n: number) => serializedSchemaBytes(generated(n).schema);
    const shipped = (n: number) =>
      serializedSchemaBytes(shippedSchema(generated(n), "functions/quote.ts"));

    // Fits bare, exactly at the cap; the comment takes it over.
    const fitsBare = FUNCTION_SCHEMA_MAX_BYTES - bare(1) + 1;
    expect(bare(fitsBare)).toBe(FUNCTION_SCHEMA_MAX_BYTES);
    expect(shipped(fitsBare)).toBeGreaterThan(FUNCTION_SCHEMA_MAX_BYTES);
    const refused = embedFunctionSchemas(APP, bundle, MANIFEST, reader(fitsBare));
    expect(refused.generated).toEqual([]);
    expect(refused.warnings).toEqual([
      expect.stringMatching(/"quote": the inputSchema .* over the 16384-byte cap/)
    ]);
    expect(bundled().quote?.inputSchema).toBeUndefined();

    // Exactly at the cap as shipped: written, and what is written is that size.
    const fitsShipped = fitsBare - (shipped(fitsBare) - FUNCTION_SCHEMA_MAX_BYTES);
    expect(shipped(fitsShipped)).toBe(FUNCTION_SCHEMA_MAX_BYTES);
    const written = embedFunctionSchemas(APP, bundle, MANIFEST, reader(fitsShipped));
    expect(written.generated).toEqual(["quote: inputSchema from `Input`"]);
    expect(serializedSchemaBytes(bundled().quote?.inputSchema)).toBe(FUNCTION_SCHEMA_MAX_BYTES);
  });
});

describe("appSourceReader", () => {
  it("reads inside the app directory only", () => {
    const read = appSourceReader(APP);
    expect(read(join(APP, "functions", "types.ts"))).toContain("interface Order");
    expect(
      read(join(APP, "..", "function-lint", "imports", "functions", "entry.ts"))
    ).toBeUndefined();
    expect(read(join(APP, "node_modules", "x", "index.ts"))).toBeUndefined();
    expect(read(join(APP, "functions"))).toBeUndefined();
  });
});

describe("functionTypesFromSource", () => {
  it("handles an arrow handler with an object-literal return type", () => {
    const files: Record<string, string> = {
      "/app/functions/fn.ts": `
        type Body = { id: string };
        export default async (req: unknown, _ctx: unknown): Promise<{ ok: boolean }> => {
          const b = JSON.parse((req as { body: string }).body) as Body;
          return { ok: b.id !== "" };
        };
      `
    };
    const types = functionTypesFromSource("/app/functions/fn.ts", (p) => files[p], "/app");
    expect(types.output?.schema).toMatchObject({ properties: { ok: { type: "boolean" } } });
    // `(req as …).body` is not `req.body`: the cast is not read as the input.
    expect(types.input).toBeUndefined();
    expect(types.problems).toEqual([]);
  });
});
