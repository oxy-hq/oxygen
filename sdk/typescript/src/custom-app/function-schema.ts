// A function's `inputSchema` / `outputSchema` in `oxy-app.json`: JSON Schema for
// the body it is called with and the value it returns, in a SUBSET of draft
// 2020-12 that every reader agrees on without a schema engine.
//
// The platform refuses a publish whose schema breaks these rules; this is the
// same check, said first — by `oxyc validate`, `oxyc publish` before it builds,
// and the Vite plugin. The keyword list, the size cap and the depth cap are the
// platform's, so a schema that passes here passes there.
//
// | Kind        | Keywords |
// | ----------- | -------- |
// | annotations | `$schema` (root only, the 2020-12 dialect), `$comment`, `title`, `description`, `default`, `examples`, `deprecated`, `format` |
// | any type    | `type` (a name or a list of names), `enum`, `const`, `anyOf`, `oneOf` |
// | object      | `properties`, `required`, `additionalProperties` (a boolean or a schema) |
// | array       | `items` (one schema), `minItems`, `maxItems`, `uniqueItems` |
// | string      | `minLength`, `maxLength`, `pattern` (ECMA-262) |
// | number      | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf` |
//
// No `$ref` / `$defs`: a schema is a finite tree, which is what lets a console
// render it as a field table and check an input against it in a few lines.
//
// Pure and dependency-free; nothing past ES2020.

/** The manifest keys a function declares its schemas under. */
export const FUNCTION_SCHEMA_KEYS = ["inputSchema", "outputSchema"] as const;

/** Largest schema per function and direction, as compact JSON in UTF-8 bytes. */
export const FUNCTION_SCHEMA_MAX_BYTES = 16 * 1024;

/** Deepest nesting of subschemas. */
export const FUNCTION_SCHEMA_MAX_DEPTH = 12;

/** The one dialect `$schema` may name. */
export const FUNCTION_SCHEMA_DIALECT = "https://json-schema.org/draft/2020-12/schema";

/** The JSON Schema type names. */
export const FUNCTION_SCHEMA_TYPES = [
  "object",
  "array",
  "string",
  "number",
  "integer",
  "boolean",
  "null"
] as const;

/** Every keyword the subset reads. */
export const FUNCTION_SCHEMA_KEYWORDS = [
  "$schema",
  "$comment",
  "title",
  "description",
  "default",
  "examples",
  "deprecated",
  "format",
  "type",
  "enum",
  "const",
  "anyOf",
  "oneOf",
  "properties",
  "required",
  "additionalProperties",
  "items",
  "minItems",
  "maxItems",
  "uniqueItems",
  "minLength",
  "maxLength",
  "pattern",
  "minimum",
  "maximum",
  "exclusiveMinimum",
  "exclusiveMaximum",
  "multipleOf"
] as const;

/** What is wrong with one schema, and where inside it (`[]` for its root). */
export interface FunctionSchemaProblem {
  path: string[];
  message: string;
}

/** A finding about `functions.<name>.inputSchema` / `outputSchema`. */
export interface FunctionSchemaFinding {
  level: "error";
  /** Dotted, from the manifest root: `functions.quote.inputSchema.properties.store`. */
  path: string;
  message: string;
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const KNOWN_TYPES: readonly string[] = FUNCTION_SCHEMA_TYPES;
const REFERENCES = ["$ref", "$defs", "definitions", "$dynamicRef", "$anchor"];

/** UTF-8 length of compact JSON — what the server measures. */
export function serializedSchemaBytes(schema: unknown): number {
  const text = JSON.stringify(schema) ?? "";
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code < 0x80) bytes += 1;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff) {
      bytes += 4;
      i++;
    } else bytes += 3;
  }
  return bytes;
}

/**
 * The first problem with `schema` as a schema in the supported subset, or
 * `undefined` when there is none. Size is {@link checkFunctionSchemas}'.
 */
export function checkFunctionSchema(schema: unknown): FunctionSchemaProblem | undefined {
  return walk(schema, [], 0);
}

/** Every declared function schema's first problem, as manifest findings. */
export function checkFunctionSchemas(manifest: unknown): FunctionSchemaFinding[] {
  const functions = isRecord(manifest) && isRecord(manifest.functions) ? manifest.functions : {};
  const out: FunctionSchemaFinding[] = [];
  for (const [name, spec] of Object.entries(functions)) {
    if (!isRecord(spec)) continue;
    for (const key of FUNCTION_SCHEMA_KEYS) {
      const schema = spec[key];
      if (schema === undefined || schema === null) continue;
      const at = `functions.${name}.${key}`;
      const size = serializedSchemaBytes(schema);
      if (size > FUNCTION_SCHEMA_MAX_BYTES) {
        out.push({
          level: "error",
          path: at,
          message:
            `function ${JSON.stringify(name)} \`${key}\` is ${size} bytes; a function schema may be at ` +
            `most ${FUNCTION_SCHEMA_MAX_BYTES} — trim long descriptions and examples`
        });
        continue;
      }
      const problem = checkFunctionSchema(schema);
      if (problem) {
        out.push({
          level: "error",
          path: [at, ...problem.path].join("."),
          message: `function ${JSON.stringify(name)} \`${[key, ...problem.path].join(".")}\` ${problem.message}`
        });
      }
    }
  }
  return out;
}

function walk(schema: unknown, path: string[], depth: number): FunctionSchemaProblem | undefined {
  if (depth > FUNCTION_SCHEMA_MAX_DEPTH) {
    return { path, message: `is nested deeper than ${FUNCTION_SCHEMA_MAX_DEPTH} levels` };
  }
  if (!isRecord(schema)) {
    return { path, message: 'must be a JSON Schema object ({ "type": ... })' };
  }
  for (const [key, value] of Object.entries(schema)) {
    const problem = keyword(key, value, [...path, key], path.length === 0, depth);
    if (problem) return problem;
  }
  return undefined;
}

const sub = (value: unknown, path: string[], depth: number): FunctionSchemaProblem | undefined =>
  walk(value, path, depth + 1);

const isWhole = (value: unknown): boolean =>
  typeof value === "number" && Number.isInteger(value) && value >= 0;

function keyword(
  key: string,
  value: unknown,
  path: string[],
  root: boolean,
  depth: number
): FunctionSchemaProblem | undefined {
  const fail = (message: string): FunctionSchemaProblem => ({ path, message });
  switch (key) {
    case "$schema":
      if (!root) return fail("belongs on the root only");
      return value === FUNCTION_SCHEMA_DIALECT
        ? undefined
        : fail(
            `must be ${JSON.stringify(FUNCTION_SCHEMA_DIALECT)} — the subset is of draft 2020-12`
          );
    case "$comment":
    case "title":
    case "description":
    case "format":
      return typeof value === "string" ? undefined : fail("must be text");
    case "pattern":
      if (typeof value !== "string") return fail("must be text");
      try {
        new RegExp(value, "u");
        return undefined;
      } catch {
        return fail(`${JSON.stringify(value)} is not a valid regular expression`);
      }
    case "default":
    case "const":
      return undefined;
    case "examples":
      return Array.isArray(value) ? undefined : fail("must be a list of example values");
    case "deprecated":
    case "uniqueItems":
      return typeof value === "boolean" ? undefined : fail("must be true or false");
    case "type":
      return typeProblem(value, path);
    case "enum":
      return Array.isArray(value) && value.length > 0
        ? undefined
        : fail("must be a non-empty list of values");
    case "anyOf":
    case "oneOf": {
      if (!Array.isArray(value) || value.length === 0) {
        return fail("must be a non-empty list of schemas");
      }
      for (let i = 0; i < value.length; i++) {
        const problem = sub(value[i], [...path, String(i)], depth);
        if (problem) return problem;
      }
      return undefined;
    }
    case "properties": {
      if (!isRecord(value)) return fail("must be an object mapping each field to its schema");
      for (const [name, member] of Object.entries(value)) {
        const problem = sub(member, [...path, name], depth);
        if (problem) return problem;
      }
      return undefined;
    }
    case "required": {
      if (!Array.isArray(value)) return fail("must be a list of field names");
      const seen = new Set<string>();
      for (const name of value) {
        if (typeof name !== "string") return fail("must list field names as strings");
        if (seen.has(name)) return fail(`lists ${JSON.stringify(name)} twice`);
        seen.add(name);
      }
      return undefined;
    }
    case "additionalProperties":
      return typeof value === "boolean" ? undefined : sub(value, path, depth);
    case "items":
      if (Array.isArray(value)) {
        return fail("must be one schema for every item; tuples (`prefixItems`) are not supported");
      }
      return sub(value, path, depth);
    case "minItems":
    case "maxItems":
    case "minLength":
    case "maxLength":
      return isWhole(value) ? undefined : fail("must be a whole number, 0 or more");
    case "minimum":
    case "maximum":
    case "exclusiveMinimum":
    case "exclusiveMaximum":
      return typeof value === "number" && Number.isFinite(value)
        ? undefined
        : fail("must be a number");
    case "multipleOf":
      return typeof value === "number" && value > 0
        ? undefined
        : fail("must be a number greater than 0");
    case "nullable":
      return fail('is OpenAPI, not JSON Schema: add "null" to `type` instead');
    default:
      // Every keyword the subset reads has its case above, so this is one it
      // does not: refused by name, never skipped — a keyword nobody enforces
      // is a constraint the author believes holds.
      return REFERENCES.includes(key)
        ? fail(
            "is not supported: write the referenced schema inline (the subset has no references)"
          )
        : fail(`is not in the supported subset (${FUNCTION_SCHEMA_KEYWORDS.join(", ")})`);
  }
}

function typeProblem(value: unknown, path: string[]): FunctionSchemaProblem | undefined {
  const unknown = (name: string): FunctionSchemaProblem => ({
    path,
    message: `${JSON.stringify(name)} is not a JSON Schema type (one of ${FUNCTION_SCHEMA_TYPES.join(", ")})`
  });
  if (typeof value === "string") return KNOWN_TYPES.includes(value) ? undefined : unknown(value);
  if (!Array.isArray(value) || value.length === 0) {
    return { path, message: "must be a type name or a non-empty list of them" };
  }
  const seen = new Set<string>();
  for (const name of value) {
    if (typeof name !== "string") return { path, message: "must list type names as strings" };
    if (!KNOWN_TYPES.includes(name)) return unknown(name);
    if (seen.has(name)) return { path, message: `lists ${JSON.stringify(name)} twice` };
    seen.add(name);
  }
  return undefined;
}
