/**
 * A function's `inputSchema` / `outputSchema` as the console reads them: a field
 * table, and a check of Run now's input before it is sent.
 *
 * The schemas are a SUBSET of JSON Schema 2020-12, refused at publish otherwise
 * (`crates/api-custom-apps/src/custom_apps_function_schema.rs`, mirrored for
 * `oxyc` by `sdk/typescript/src/custom-app/function-schema.ts`): no `$ref`, so a
 * schema is a finite tree this walks directly. The keywords read here are that
 * subset's; `format` is an annotation, as 2020-12 makes it, and is shown but not
 * asserted. web-app has no JSON Schema validator in its dependencies, and the
 * subset is small enough not to want one.
 */

export type JsonSchema = Record<string, unknown>;

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const schemaOf = (value: unknown): JsonSchema | undefined => (isRecord(value) ? value : undefined);

// ── Field table ─────────────────────────────────────────────────────────────

/** One row of the table: a field, flattened to a path. */
export interface SchemaField {
  /** `store`, `customer.email`, `lines[].qty`; `(value)` for a non-object root. */
  name: string;
  /** A compact type: `integer`, `string · date-time`, `"a" | "b"`, `string[]`. */
  type: string;
  required: boolean;
  description?: string;
}

/** How deep nested objects are flattened into rows before stopping. */
const FIELD_DEPTH = 3;

/** At most this many enum values are spelled out. */
const ENUM_SHOWN = 4;

/** A schema's type, as one short label. */
export function typeLabel(schema: JsonSchema | undefined): string {
  if (!schema) return "any";
  if (Array.isArray(schema.enum)) {
    const shown = schema.enum.slice(0, ENUM_SHOWN).map((v) => JSON.stringify(v));
    return schema.enum.length > ENUM_SHOWN ? `${shown.join(" | ")} | …` : shown.join(" | ");
  }
  if ("const" in schema) return JSON.stringify(schema.const);
  const branches = Array.isArray(schema.anyOf)
    ? schema.anyOf
    : Array.isArray(schema.oneOf)
      ? schema.oneOf
      : undefined;
  if (branches) return branches.map((b) => typeLabel(schemaOf(b))).join(" | ");
  const types = Array.isArray(schema.type)
    ? (schema.type as unknown[]).filter((t): t is string => typeof t === "string")
    : typeof schema.type === "string"
      ? [schema.type]
      : [];
  if (types.length === 0) return "any";
  const labels = types.map((t) => {
    if (t === "array") {
      const item = typeLabel(schemaOf(schema.items));
      return item.includes(" | ") ? `(${item})[]` : `${item}[]`;
    }
    if (t === "string" && typeof schema.format === "string") return `string · ${schema.format}`;
    return t;
  });
  return labels.join(" | ");
}

/** The object schema whose fields a row's children are, if any. */
function objectFields(schema: JsonSchema): JsonSchema | undefined {
  if (isRecord(schema.properties)) return schema;
  const items = schemaOf(schema.items);
  if (items && isRecord(items.properties)) return items;
  return undefined;
}

/**
 * The rows a schema shows: each property, its nested properties under a dotted
 * path (an array of objects as `name[].field`), to {@link FIELD_DEPTH} levels.
 * A root that is not an object with fields is one `(value)` row.
 */
export function schemaFields(schema: JsonSchema | null | undefined): SchemaField[] {
  if (!schema) return [];
  if (!isRecord(schema.properties)) {
    return [
      {
        name: "(value)",
        type: typeLabel(schema),
        required: true,
        description: typeof schema.description === "string" ? schema.description : undefined
      }
    ];
  }
  const rows: SchemaField[] = [];
  const visit = (object: JsonSchema, prefix: string, depth: number) => {
    const required = Array.isArray(object.required) ? object.required : [];
    for (const [name, raw] of Object.entries(object.properties as Record<string, unknown>)) {
      const field = schemaOf(raw) ?? {};
      const path = `${prefix}${name}`;
      rows.push({
        name: path,
        type: typeLabel(field),
        required: required.includes(name),
        description: typeof field.description === "string" ? field.description : undefined
      });
      const nested = objectFields(field);
      if (nested && depth < FIELD_DEPTH) {
        visit(nested, nested === field ? `${path}.` : `${path}[].`, depth + 1);
      }
    }
  };
  visit(schema, "", 1);
  return rows;
}

// ── Checking Run now's input ────────────────────────────────────────────────

/** One reason a value does not fit, already a sentence. */
export type SchemaError = string;

/** Stop collecting past this many: the panel shows the first few. */
const MAX_ERRORS = 20;

/** Depth guard; a published schema is at most 12 deep, a value may be deeper. */
const MAX_DEPTH = 32;

/** Stable JSON, keys sorted, for `enum` / `const` / `uniqueItems` equality. */
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (isRecord(value)) {
    const keys = Object.keys(value).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`).join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

function hasType(type: string, value: unknown): boolean {
  switch (type) {
    case "object":
      return isRecord(value);
    case "array":
      return Array.isArray(value);
    case "string":
      return typeof value === "string";
    case "boolean":
      return typeof value === "boolean";
    case "null":
      return value === null;
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    default:
      return true;
  }
}

const where = (path: string): string => (path === "" ? "the input" : path);

const num = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;

/**
 * Every way `value` does not fit `schema` (up to {@link MAX_ERRORS}), each a
 * sentence naming where: `lines[0].qty must be at least 1`. Empty when it fits.
 */
export function validateAgainstSchema(schema: JsonSchema, value: unknown): SchemaError[] {
  const errors: SchemaError[] = [];
  check(schema, value, "", 0, errors);
  return errors.slice(0, MAX_ERRORS);
}

function check(
  schema: JsonSchema,
  value: unknown,
  path: string,
  depth: number,
  out: SchemaError[]
) {
  if (out.length >= MAX_ERRORS || depth > MAX_DEPTH) return;
  const at = where(path);
  const push = (message: string) => out.push(`${at} ${message}`);

  const types = Array.isArray(schema.type)
    ? (schema.type as string[])
    : typeof schema.type === "string"
      ? [schema.type]
      : undefined;
  if (types && !types.some((t) => hasType(t, value))) {
    push(`must be ${types.map((t) => (t === "integer" ? "an integer" : t)).join(" or ")}`);
    return;
  }
  if (Array.isArray(schema.enum) && !schema.enum.some((v) => canonical(v) === canonical(value))) {
    push(`must be one of ${schema.enum.map((v) => JSON.stringify(v)).join(", ")}`);
    return;
  }
  if ("const" in schema && canonical(schema.const) !== canonical(value)) {
    push(`must be ${JSON.stringify(schema.const)}`);
    return;
  }
  for (const key of ["anyOf", "oneOf"] as const) {
    const branches = schema[key];
    if (!Array.isArray(branches)) continue;
    const fits = branches.filter((b) => {
      const scratch: SchemaError[] = [];
      check(schemaOf(b) ?? {}, value, path, depth + 1, scratch);
      return scratch.length === 0;
    }).length;
    if (key === "anyOf" && fits === 0) push("does not match any of the allowed shapes");
    if (key === "oneOf" && fits !== 1) {
      push(
        fits === 0
          ? "does not match any of the allowed shapes"
          : "matches more than one allowed shape"
      );
    }
  }

  if (typeof value === "string") {
    // JSON Schema counts a string's length in code points, which is exactly what
    // spreading a string yields — not graphemes, and not UTF-16 units.
    // oxlint-disable-next-line typescript/no-misused-spread -- code points are the spec's unit here
    const length = [...value].length;
    const min = num(schema.minLength);
    const max = num(schema.maxLength);
    if (min !== undefined && length < min) push(`must be at least ${min} characters`);
    if (max !== undefined && length > max) push(`must be at most ${max} characters`);
    if (typeof schema.pattern === "string") {
      let re: RegExp | undefined;
      try {
        re = new RegExp(schema.pattern, "u");
      } catch {
        re = undefined; // publish checks the pattern; nothing to assert against here
      }
      if (re && !re.test(value)) push(`must match ${schema.pattern}`);
    }
  } else if (typeof value === "number") {
    const min = num(schema.minimum);
    const max = num(schema.maximum);
    const xmin = num(schema.exclusiveMinimum);
    const xmax = num(schema.exclusiveMaximum);
    const step = num(schema.multipleOf);
    if (min !== undefined && value < min) push(`must be at least ${min}`);
    if (max !== undefined && value > max) push(`must be at most ${max}`);
    if (xmin !== undefined && value <= xmin) push(`must be greater than ${xmin}`);
    if (xmax !== undefined && value >= xmax) push(`must be less than ${xmax}`);
    if (step !== undefined && step > 0) {
      const ratio = value / step;
      if (Math.abs(ratio - Math.round(ratio)) > 1e-9) push(`must be a multiple of ${step}`);
    }
  } else if (Array.isArray(value)) {
    const min = num(schema.minItems);
    const max = num(schema.maxItems);
    if (min !== undefined && value.length < min) push(`must have at least ${min} items`);
    if (max !== undefined && value.length > max) push(`must have at most ${max} items`);
    if (schema.uniqueItems === true) {
      const seen = new Set(value.map(canonical));
      if (seen.size !== value.length) push("must not repeat an item");
    }
    const items = schemaOf(schema.items);
    if (items) {
      for (const [i, item] of value.entries()) check(items, item, `${path}[${i}]`, depth + 1, out);
    }
  } else if (isRecord(value)) {
    const properties = isRecord(schema.properties) ? schema.properties : {};
    const prefix = path === "" ? "" : `${path}.`;
    if (Array.isArray(schema.required)) {
      for (const name of schema.required) {
        if (typeof name === "string" && value[name] === undefined) {
          out.push(`${prefix}${name} is required`);
        }
      }
    }
    for (const [name, member] of Object.entries(value)) {
      const declared = schemaOf(properties[name]);
      if (declared) {
        check(declared, member, `${prefix}${name}`, depth + 1, out);
      } else if (schema.additionalProperties === false) {
        out.push(`${prefix}${name} is not a field this function takes`);
      } else {
        const extra = schemaOf(schema.additionalProperties);
        if (extra) check(extra, member, `${prefix}${name}`, depth + 1, out);
      }
    }
  }
}

/**
 * What Run now does with the box: send `input`, or show `errors` instead.
 * `bySchema` failures may be overridden (the schema is advisory at run time);
 * invalid JSON may not, since there is nothing to send.
 */
export type RunInputCheck =
  | { ok: true; input: unknown }
  | { ok: false; errors: SchemaError[]; bySchema: boolean };

/**
 * The params box, parsed and checked against the input schema.
 *
 * Empty sends no params. With a schema whose root requires fields that is
 * refused too, naming them: the function would receive none of them.
 */
export function checkRunInput(text: string, schema: JsonSchema | null | undefined): RunInputCheck {
  const trimmed = text.trim();
  if (trimmed === "") {
    const required = schema && Array.isArray(schema.required) ? schema.required : [];
    if (required.length === 0) return { ok: true, input: undefined };
    return {
      ok: false,
      errors: [
        `the box is empty, and the input schema requires ${required.map(String).join(", ")}`
      ],
      bySchema: true
    };
  }
  let input: unknown;
  try {
    input = JSON.parse(trimmed);
  } catch {
    return { ok: false, errors: ["the input is not valid JSON"], bySchema: false };
  }
  if (!schema) return { ok: true, input };
  const errors = validateAgainstSchema(schema, input);
  return errors.length === 0 ? { ok: true, input } : { ok: false, errors, bySchema: true };
}
