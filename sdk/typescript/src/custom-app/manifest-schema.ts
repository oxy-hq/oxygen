// The generated JSON Schema for `oxy-app.json`, as a value.
//
// `schema/oxy-app.schema.json` is written by `scripts/gen-manifest-schema.mjs`
// from `manifest-types.ts`; importing it here is what lets every package
// validate against the same document without reading a file at run time —
// the bundlers inline it.

import schema from "../../schema/oxy-app.schema.json";

/**
 * A JSON Schema node, as loosely as the walker in `manifest-validate.ts`
 * needs it. The generator emits draft-07 with `definitions`; the keywords it
 * uses are pinned by `manifest-schema.test.ts`, so a new one cannot arrive
 * unvalidated.
 */
export interface ManifestSchemaNode {
  $ref?: string;
  type?: string | string[];
  properties?: Record<string, ManifestSchemaNode>;
  required?: string[];
  additionalProperties?: boolean | ManifestSchemaNode;
  propertyNames?: { pattern?: string };
  items?: ManifestSchemaNode;
  maxItems?: number;
  uniqueItems?: boolean;
  enum?: unknown[];
  const?: unknown;
  anyOf?: ManifestSchemaNode[];
  pattern?: string;
  minLength?: number;
  maxLength?: number;
  minimum?: number;
  maximum?: number;
  description?: string;
}

/** The document: a root `$ref` into `definitions`. */
export interface ManifestSchemaDocument extends ManifestSchemaNode {
  $schema: string;
  $id: string;
  title: string;
  definitions: Record<string, ManifestSchemaNode>;
}

/** The schema `oxy-app.json` is validated against — `schema/oxy-app.schema.json`. */
export const OXY_APP_SCHEMA = schema as ManifestSchemaDocument;

/**
 * What a manifest's `$schema` points at: the published copy. An app that wants
 * the installed SDK's exact copy points at
 * `./node_modules/@oxy-hq/sdk/schema/oxy-app.schema.json` instead.
 */
export const OXY_APP_SCHEMA_URL = OXY_APP_SCHEMA.$id;

/** The schema's file name, as `oxyc validate` names the schema a file is checked by. */
export const OXY_APP_SCHEMA_FILE = "oxy-app.schema.json";
