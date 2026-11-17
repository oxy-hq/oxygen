import { describe, expect, it } from "vitest";
import {
  checkFunctionSchema,
  checkFunctionSchemas,
  FUNCTION_SCHEMA_DIALECT,
  FUNCTION_SCHEMA_MAX_BYTES,
  FUNCTION_SCHEMA_MAX_DEPTH,
  serializedSchemaBytes
} from "./function-schema";
import { validateOxyAppManifest } from "./manifest-validate";

/** Every supported keyword at least once — the Rust gate's `full_schema` too. */
const FULL = {
  $schema: FUNCTION_SCHEMA_DIALECT,
  $comment: "generated",
  title: "Input",
  description: "Create an order.",
  type: "object",
  properties: {
    store: { type: "integer", minimum: 1, maximum: 9999, description: "Store number." },
    note: { type: ["string", "null"], maxLength: 200, minLength: 0, pattern: "^\\S" },
    kind: { enum: ["pickup", "delivery"], default: "pickup" },
    placed_at: { type: "string", format: "date-time", examples: ["2026-11-17T10:00:00Z"] },
    lines: {
      type: "array",
      minItems: 1,
      maxItems: 50,
      uniqueItems: false,
      items: {
        type: "object",
        properties: {
          sku: { type: "string" },
          qty: { type: "number", exclusiveMinimum: 0, multipleOf: 0.5 }
        },
        required: ["sku", "qty"],
        additionalProperties: false
      }
    },
    tags: { type: "object", additionalProperties: { type: "string" } },
    version: { const: 2, deprecated: true },
    who: { anyOf: [{ type: "string" }, { type: "integer" }] },
    when: { oneOf: [{ type: "string" }, { type: "null" }], exclusiveMaximum: 3 }
  },
  required: ["store", "lines"],
  additionalProperties: true
};

const manifest = (fn: Record<string, unknown>) => ({
  schemaVersion: 2,
  slug: "orders",
  functions: { quote: { route: true, ...fn } }
});

describe("checkFunctionSchema", () => {
  it("accepts every supported keyword, and the empty schema", () => {
    expect(checkFunctionSchema(FULL)).toBeUndefined();
    expect(checkFunctionSchema({})).toBeUndefined();
  });

  it("names an unknown keyword and where it is", () => {
    const problem = checkFunctionSchema({
      type: "object",
      properties: { store: { type: "integer", minimun: 1 } }
    });
    expect(problem?.path).toEqual(["properties", "store", "minimun"]);
    expect(problem?.message).toMatch(/is not in the supported subset/);
  });

  it("refuses references, nullable, tuples and unsupported composition", () => {
    expect(checkFunctionSchema({ $ref: "#/$defs/a" })?.message).toMatch(/inline/);
    expect(checkFunctionSchema({ type: "string", nullable: true })?.message).toMatch(
      /add "null" to `type`/
    );
    expect(checkFunctionSchema({ items: [{}] })?.message).toMatch(/tuples/);
    for (const bad of [{ allOf: [{}] }, { not: {} }, { if: {} }, { patternProperties: {} }]) {
      expect(checkFunctionSchema(bad), JSON.stringify(bad)).toBeDefined();
    }
  });

  it("checks keyword values", () => {
    const bad: unknown[] = [
      "object",
      true,
      [],
      { type: "int" },
      { type: [] },
      { type: ["string", "string"] },
      { enum: [] },
      { anyOf: [] },
      { properties: [] },
      { required: ["a", "a"] },
      { required: [1] },
      { minLength: -1 },
      { maxItems: 1.5 },
      { minimum: "1" },
      { multipleOf: 0 },
      { pattern: "(" },
      { description: 1 },
      { examples: "x" },
      { additionalProperties: "no" },
      { $schema: "http://json-schema.org/draft-07/schema#" },
      { properties: { a: { $schema: FUNCTION_SCHEMA_DIALECT } } }
    ];
    for (const schema of bad) {
      expect(checkFunctionSchema(schema), JSON.stringify(schema)).toBeDefined();
    }
  });

  it("caps nesting at the server's depth", () => {
    let ok: Record<string, unknown> = { type: "string" };
    for (let i = 0; i < FUNCTION_SCHEMA_MAX_DEPTH; i++) ok = { type: "array", items: ok };
    expect(checkFunctionSchema(ok)).toBeUndefined();
    expect(checkFunctionSchema({ type: "array", items: ok })?.message).toMatch(/nested deeper/);
  });
});

describe("checkFunctionSchemas", () => {
  it("measures UTF-8 bytes of compact JSON, as the server does", () => {
    expect(serializedSchemaBytes({ a: "é" })).toBe(JSON.stringify({ a: "x" }).length + 1);
    expect(serializedSchemaBytes({ a: "😀" })).toBe(JSON.stringify({ a: "" }).length + 4);
  });

  it("refuses a schema over the cap, inclusive", () => {
    const base = { type: "string", description: "" };
    const room = FUNCTION_SCHEMA_MAX_BYTES - serializedSchemaBytes(base);
    const exact = { type: "string", description: "x".repeat(room) };
    expect(checkFunctionSchemas(manifest({ outputSchema: exact }))).toEqual([]);
    const over = { type: "string", description: "x".repeat(room + 1) };
    const [finding] = checkFunctionSchemas(manifest({ outputSchema: over }));
    expect(finding?.path).toBe("functions.quote.outputSchema");
    expect(finding?.message).toMatch(/at most 16384/);
  });

  it("reports the first problem per schema with a dotted manifest path", () => {
    const findings = checkFunctionSchemas(
      manifest({
        inputSchema: { type: "object", properties: { id: { type: "uuid" } } },
        outputSchema: { $ref: "#/x" }
      })
    );
    expect(findings.map((f) => f.path)).toEqual([
      "functions.quote.inputSchema.properties.id.type",
      "functions.quote.outputSchema.$ref"
    ]);
  });

  it("skips functions without schemas and a null schema", () => {
    expect(checkFunctionSchemas(manifest({}))).toEqual([]);
    expect(checkFunctionSchemas(manifest({ inputSchema: null }))).toEqual([]);
    expect(checkFunctionSchemas({})).toEqual([]);
  });
});

describe("validateOxyAppManifest on function schemas", () => {
  it("accepts a valid schema with no warnings", () => {
    expect(validateOxyAppManifest(manifest({ inputSchema: FULL, outputSchema: FULL }))).toEqual([]);
  });

  it("makes an unknown keyword an error, said once, not the walker's warning", () => {
    const issues = validateOxyAppManifest(
      manifest({ inputSchema: { type: "object", require: ["a"] } })
    );
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({
      level: "error",
      path: "functions.quote.inputSchema.require"
    });
  });

  it("refuses a schema that is not an object", () => {
    const issues = validateOxyAppManifest(manifest({ inputSchema: "object" }));
    expect(issues).toEqual([
      {
        level: "error",
        path: "functions.quote.inputSchema",
        message: expect.stringMatching(/must be a JSON Schema object/)
      }
    ]);
  });
});
