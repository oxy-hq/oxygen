import { describe, expect, it } from "vitest";
import { checkRunInput, schemaFields, typeLabel, validateAgainstSchema } from "./functionSchema";

/** What `oxyc publish` writes for a typical `Input`. */
const ORDER = {
  type: "object",
  properties: {
    store: { type: "integer", minimum: 1, description: "Store number." },
    kind: { type: "string", enum: ["pickup", "delivery"] },
    note: { type: ["string", "null"], maxLength: 5 },
    at: { type: "string", format: "date-time" },
    lines: {
      type: "array",
      minItems: 1,
      items: {
        type: "object",
        properties: {
          sku: { type: "string", pattern: "^[A-Z]+$" },
          qty: { type: "number", exclusiveMinimum: 0, multipleOf: 0.5 }
        },
        required: ["sku", "qty"],
        additionalProperties: false
      }
    },
    customer: {
      type: "object",
      properties: { email: { type: "string" } },
      required: ["email"]
    },
    tags: { type: "array", items: { type: "string" }, uniqueItems: true },
    who: { anyOf: [{ type: "string" }, { type: "integer" }] }
  },
  required: ["store", "lines"],
  additionalProperties: false
};

describe("schemaFields", () => {
  it("flattens nested objects and arrays of objects into named rows", () => {
    expect(schemaFields(ORDER)).toEqual([
      { name: "store", type: "integer", required: true, description: "Store number." },
      { name: "kind", type: '"pickup" | "delivery"', required: false, description: undefined },
      { name: "note", type: "string | null", required: false, description: undefined },
      { name: "at", type: "string · date-time", required: false, description: undefined },
      { name: "lines", type: "object[]", required: true, description: undefined },
      { name: "lines[].sku", type: "string", required: true, description: undefined },
      { name: "lines[].qty", type: "number", required: true, description: undefined },
      { name: "customer", type: "object", required: false, description: undefined },
      { name: "customer.email", type: "string", required: true, description: undefined },
      { name: "tags", type: "string[]", required: false, description: undefined },
      { name: "who", type: "string | integer", required: false, description: undefined }
    ]);
  });

  it("shows a non-object root as one value row, and nothing for no schema", () => {
    expect(
      schemaFields({ type: "array", items: { type: "string" }, description: "Names." })
    ).toEqual([{ name: "(value)", type: "string[]", required: true, description: "Names." }]);
    expect(schemaFields(null)).toEqual([]);
    expect(schemaFields({})).toEqual([{ name: "(value)", type: "any", required: true }]);
  });

  it("stops flattening three levels down", () => {
    const deep = (n: number): Record<string, unknown> =>
      n === 0 ? { type: "string" } : { type: "object", properties: { a: deep(n - 1) } };
    expect(schemaFields(deep(5)).map((f) => f.name)).toEqual(["a", "a.a", "a.a.a"]);
  });
});

describe("typeLabel", () => {
  it("abbreviates a long enum and brackets an array of a union", () => {
    expect(typeLabel({ enum: [1, 2, 3, 4, 5] })).toBe("1 | 2 | 3 | 4 | …");
    expect(typeLabel({ type: "array", items: { type: ["string", "null"] } })).toBe(
      "(string | null)[]"
    );
    expect(typeLabel({ const: "v2" })).toBe('"v2"');
    expect(typeLabel(undefined)).toBe("any");
  });
});

describe("validateAgainstSchema", () => {
  const good = { store: 3, kind: "pickup", lines: [{ sku: "AB", qty: 1.5 }] };

  it("accepts a value that fits", () => {
    expect(validateAgainstSchema(ORDER, good)).toEqual([]);
    expect(validateAgainstSchema({}, "anything")).toEqual([]);
  });

  it("names each misfit by path", () => {
    expect(
      validateAgainstSchema(ORDER, {
        store: 0,
        kind: "drone",
        note: "too long",
        lines: [
          { sku: "ab", qty: 0.3 },
          { qty: -1, extra: true }
        ],
        customer: {},
        tags: ["a", "a"],
        who: false,
        surprise: 1
      })
    ).toEqual([
      "store must be at least 1",
      'kind must be one of "pickup", "delivery"',
      "note must be at most 5 characters",
      "lines[0].sku must match ^[A-Z]+$",
      "lines[0].qty must be a multiple of 0.5",
      "lines[1].sku is required",
      "lines[1].qty must be greater than 0",
      "lines[1].extra is not a field this function takes",
      "customer.email is required",
      "tags must not repeat an item",
      "who does not match any of the allowed shapes",
      "surprise is not a field this function takes"
    ]);
  });

  it("checks the root's type, integers, null and required fields", () => {
    expect(validateAgainstSchema(ORDER, [1])).toEqual(["the input must be object"]);
    expect(validateAgainstSchema(ORDER, { ...good, store: 1.5 })).toEqual([
      "store must be an integer"
    ]);
    expect(validateAgainstSchema(ORDER, { ...good, note: null })).toEqual([]);
    expect(validateAgainstSchema(ORDER, { kind: "pickup" })).toEqual([
      "store is required",
      "lines is required"
    ]);
    expect(validateAgainstSchema(ORDER, { ...good, lines: [] })).toEqual([
      "lines must have at least 1 items"
    ]);
  });

  it("reads oneOf as exactly one, and an additionalProperties schema", () => {
    const one = { oneOf: [{ type: "number" }, { type: "integer" }] };
    expect(validateAgainstSchema(one, 1.5)).toEqual([]);
    expect(validateAgainstSchema(one, 2)).toEqual([
      "the input matches more than one allowed shape"
    ]);
    const map = { type: "object", additionalProperties: { type: "boolean" } };
    expect(validateAgainstSchema(map, { a: true, b: "no" })).toEqual(["b must be boolean"]);
  });

  it("caps what it collects", () => {
    const many = Object.fromEntries(Array.from({ length: 50 }, (_, i) => [`f${i}`, 1]));
    expect(
      validateAgainstSchema({ type: "object", additionalProperties: false }, many)
    ).toHaveLength(20);
  });
});

describe("checkRunInput", () => {
  it("sends an empty box as no params unless the schema requires fields", () => {
    expect(checkRunInput("  ", null)).toEqual({ ok: true, input: undefined });
    expect(checkRunInput("", { type: "object" })).toEqual({ ok: true, input: undefined });
    expect(checkRunInput("", ORDER)).toEqual({
      ok: false,
      errors: ["the box is empty, and the input schema requires store, lines"],
      bySchema: true
    });
  });

  it("refuses invalid JSON without an override, and schema misfits with one", () => {
    expect(checkRunInput("{ nope", ORDER)).toEqual({
      ok: false,
      errors: ["the input is not valid JSON"],
      bySchema: false
    });
    expect(checkRunInput('{"store": 1}', ORDER)).toEqual({
      ok: false,
      errors: ["lines is required"],
      bySchema: true
    });
  });

  it("passes JSON through when it fits, or when there is no schema", () => {
    expect(checkRunInput('{"store":3,"lines":[{"sku":"A","qty":1}]}', ORDER)).toEqual({
      ok: true,
      input: { store: 3, lines: [{ sku: "A", qty: 1 }] }
    });
    expect(checkRunInput("[1, 2]", undefined)).toEqual({ ok: true, input: [1, 2] });
  });
});
