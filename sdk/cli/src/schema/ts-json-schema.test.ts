import { checkFunctionSchema } from "@oxy-hq/sdk/manifest";
import { describe, expect, it } from "vitest";
import {
  type JsonSchema,
  parseJsDoc,
  readTypeModule,
  SCHEMA_DIALECT,
  schemaForDeclaration,
  TypeProgram,
  UnsupportedType
} from "./ts-json-schema.js";

/** A program over in-memory files; the entry is `/app/functions/fn.ts`. */
function schemaOf(source: string, name: string, files: Record<string, string> = {}): JsonSchema {
  const all: Record<string, string> = { "/app/functions/fn.ts": source, ...files };
  const program = new TypeProgram((path) => all[path]);
  const module = program.module("/app/functions/fn.ts");
  if (!module) throw new Error("no entry");
  const schema = schemaForDeclaration(program, module, name);
  // Whatever the generator writes, the publish gate must accept.
  expect(checkFunctionSchema(schema)).toBeUndefined();
  return schema;
}

function refusal(source: string, name = "Input"): UnsupportedType {
  try {
    schemaOf(source, name);
  } catch (cause) {
    if (cause instanceof UnsupportedType) return cause;
    throw cause;
  }
  throw new Error("expected UnsupportedType");
}

describe("schemaForDeclaration", () => {
  it("turns an interface into a closed object schema, JSDoc included", () => {
    const schema = schemaOf(
      `
      import type { OxyFunctionContext } from "@oxy-hq/sdk";

      /** Ask for a delivery quote. */
      export interface Input {
        /**
         * The store to quote from.
         * @asType integer
         * @minimum 1
         */
        store: number;
        /** @maxLength 200 */
        note?: string | null;
        readonly kind: "pickup" | "delivery";
        placedAt: Date;
        tags: string[];
        lines: Array<{ sku: string; qty: number }>;
        "x-trace"?: string,
        extra: Record<string, boolean>
        anything: unknown
      }
      `,
      "Input"
    );
    expect(schema).toEqual({
      $schema: SCHEMA_DIALECT,
      description: "Ask for a delivery quote.",
      type: "object",
      properties: {
        store: { type: "integer", description: "The store to quote from.", minimum: 1 },
        note: { type: ["string", "null"], maxLength: 200 },
        kind: { type: "string", enum: ["pickup", "delivery"] },
        placedAt: { type: "string", format: "date-time" },
        tags: { type: "array", items: { type: "string" } },
        lines: {
          type: "array",
          items: {
            type: "object",
            properties: { sku: { type: "string" }, qty: { type: "number" } },
            required: ["sku", "qty"],
            additionalProperties: false
          }
        },
        "x-trace": { type: "string" },
        extra: { type: "object", additionalProperties: { type: "boolean" } },
        anything: {}
      },
      required: ["store", "kind", "placedAt", "tags", "lines", "extra", "anything"],
      additionalProperties: false
    });
  });

  it("inlines aliases and declarations, follows extends, and reads `| undefined` as optional", () => {
    const schema = schemaOf(
      `
      type Id = string;
      /** A line on an order. */
      interface Line { sku: Id; qty: number }
      interface Base { id: Id }
      export interface Order extends Base {
        lines: Line[];
        coupon: string | undefined;
        status: "open" | "closed" | null;
        flag: true | false;
        mixed: string | { a: number };
      }
      `,
      "Order"
    );
    expect(schema.required).toEqual(["id", "lines", "status", "flag", "mixed"]);
    const props = schema.properties as Record<string, JsonSchema>;
    expect(props.id).toEqual({ type: "string" });
    expect(props.lines).toEqual({
      type: "array",
      items: {
        description: "A line on an order.",
        type: "object",
        properties: { sku: { type: "string" }, qty: { type: "number" } },
        required: ["sku", "qty"],
        additionalProperties: false
      }
    });
    expect(props.coupon).toEqual({ type: "string" });
    expect(props.status).toEqual({ type: ["string", "null"], enum: ["open", "closed", null] });
    expect(props.flag).toEqual({ type: "boolean" });
    expect(props.mixed).toEqual({
      anyOf: [
        { type: "string" },
        {
          type: "object",
          properties: { a: { type: "number" } },
          required: ["a"],
          additionalProperties: false
        }
      ]
    });
    // `extends` puts the base's fields first.
    expect(Object.keys(props)).toEqual(["id", "lines", "coupon", "status", "flag", "mixed"]);
  });

  it("reads a type alias of an object, a union of objects, and literal keys", () => {
    expect(schemaOf("export type Output = { ok: boolean; count: -1 | 0 | 1 }", "Output")).toEqual({
      $schema: SCHEMA_DIALECT,
      type: "object",
      properties: { ok: { type: "boolean" }, count: { type: "number", enum: [-1, 0, 1] } },
      required: ["ok", "count"],
      additionalProperties: false
    });
    const keyed = schemaOf(`type Output = Record<"a" | "b", number>`, "Output");
    expect(keyed).toMatchObject({ required: ["a", "b"], additionalProperties: false });
    const indexed = schemaOf("interface Output { [key: string]: number }", "Output");
    expect(indexed).toMatchObject({ properties: {}, additionalProperties: { type: "number" } });
  });

  it("follows a named relative type import, and not a package one", () => {
    const files = {
      "/app/functions/types.ts": `
        /** One order. */
        export interface Order { id: string; total: number }
      `
    };
    const schema = schemaOf(
      `
      import { type Order as Row } from "./types.js";
      export interface Input { order: Row }
      `,
      "Input",
      files
    );
    expect((schema.properties as Record<string, JsonSchema>).order).toMatchObject({
      description: "One order.",
      type: "object",
      required: ["id", "total"]
    });

    const err = (() => {
      try {
        schemaOf(
          `import type { Order } from "@acme/types";\nexport interface Input { o: Order }`,
          "Input"
        );
      } catch (cause) {
        return cause as UnsupportedType;
      }
      throw new Error("expected a refusal");
    })();
    expect(err.message).toMatch(/`Order` is not declared/);
    expect(err.line).toBe(2);
  });

  it("is not confused by code, strings and comments around the declarations", () => {
    const schema = schemaOf(
      `
      const re = /interface Input { nope: x }/;
      const s = "type Input = number";
      // export interface Input { commented: true }
      function helper() { interface Input { local: string } return re.test(s); }
      export interface Input { real: string }
      export default async function fn() { return helper(); }
      `,
      "Input"
    );
    expect(Object.keys(schema.properties as object)).toEqual(["real"]);
  });

  it("names the construct and its line when a type is outside the subset", () => {
    const cases: Array<[string, RegExp]> = [
      ["export type Input = Base & { x: string };\ninterface Base { a: string }", /intersection/],
      ["export interface Input<T> { x: T }", /generic declaration/],
      ["export type Input = [string, number];", /tuple/],
      // biome-ignore lint/suspicious/noTemplateCurlyInString: TypeScript source under test
      ["export type Input = `a${string}`;", /template literal/],
      ["export interface Input { at: bigint }", /`bigint` has no JSON form/],
      ["export interface Input { run(): void }", /method `run`/],
      [
        "export interface Input { at: Partial<Base> }\ninterface Base { a: string }",
        /generic type `Partial/
      ],
      ["export interface Input { next?: Input }", /refers to itself/],
      ["export interface Input { k: keyof Base }\ninterface Base { a: string }", /`keyof`/],
      ["export interface Input {\n  a: string;\n  b: Missing;\n}", /`Missing` is not declared/]
    ];
    for (const [source, message] of cases) {
      const err = refusal(source);
      expect(err.message, source).toMatch(message);
      expect(err.file).toBe("/app/functions/fn.ts");
    }
    expect(refusal("export interface Input {\n  a: string;\n  b: Missing;\n}").line).toBe(3);
  });
});

describe("readTypeModule", () => {
  it("finds top-level declarations with their export flag and line", () => {
    const module = readTypeModule(
      "/x.ts",
      "interface A { a: string }\nexport type B = A[];\nexport declare interface C extends A, D {}\n"
    );
    expect(
      [...module.declarations.values()].map((d) => [d.name, d.kind, d.exported, d.line])
    ).toEqual([
      ["A", "interface", false, 1],
      ["B", "alias", true, 2],
      ["C", "interface", true, 3]
    ]);
    expect(module.declarations.get("C")?.heritage).toEqual(["A", "D"]);
  });
});

describe("parseJsDoc", () => {
  it("splits prose from tags and keeps paragraphs", () => {
    expect(
      parseJsDoc(`/**
       * First line
       * continues.
       *
       * Second paragraph.
       * @minimum 1
       * @pattern ^[a-z]+$
       */`)
    ).toEqual({
      description: "First line continues.\n\nSecond paragraph.",
      tags: [
        { name: "minimum", text: "1" },
        { name: "pattern", text: "^[a-z]+$" }
      ]
    });
  });
});
