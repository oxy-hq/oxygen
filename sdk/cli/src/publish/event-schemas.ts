/**
 * Event payload schemas, embedded at publish.
 *
 * In source, `oxy-app.json` points each event at a schema file:
 * `events.<name>.schema` is a path relative to the app directory. The platform
 * never reads schema files from a bundle (`internal-docs/2026-11-17-app-events-design.md`
 * §6.10), so `oxyc publish` replaces each path with the parsed JSON Schema in
 * the manifest it uploads. A schema already written inline is left as it is.
 */

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  FUNCTION_SCHEMA_MAX_BYTES,
  type OxyAppManifest,
  serializedSchemaBytes
} from "@oxy-hq/sdk/manifest";
import { CliError, ExitCode } from "../util/errors.js";

/**
 * A copy of `manifest` whose event schemas are the parsed files.
 *
 * Throws, naming the event and the path, when a schema file is missing,
 * unreadable or not JSON — a publish that shipped an event without its schema
 * would be refused by the server later and less clearly.
 */
export function embedEventSchemas(appDir: string, manifest: OxyAppManifest): OxyAppManifest {
  const events = manifest.events;
  if (events === undefined || events === null || typeof events !== "object") return manifest;

  const embedded: NonNullable<OxyAppManifest["events"]> = {};
  for (const [name, declaration] of Object.entries(events)) {
    const schema = (declaration as { schema?: unknown } | null)?.schema;
    if (typeof schema !== "string") {
      if (schema !== undefined && schema !== null) checkSize(name, schema);
      embedded[name] = declaration;
      continue;
    }
    const read = readSchema(appDir, name, schema);
    checkSize(name, read);
    embedded[name] = { ...declaration, schema: read };
  }
  return { ...manifest, events: embedded };
}

/**
 * The server's cap on an event's schema (`app_events::declared_events`): the
 * 16 KiB a function's schema may be, serialized, whether the schema was
 * written inline or read from its file.
 */
function checkSize(event: string, schema: unknown): void {
  const size = serializedSchemaBytes(schema);
  if (size > FUNCTION_SCHEMA_MAX_BYTES) {
    throw new CliError(
      `event \`${event}\`: its schema is ${size} bytes, over the ${FUNCTION_SCHEMA_MAX_BYTES}-byte cap`,
      {
        code: ExitCode.FAILURE,
        hint: "trim the schema: drop long descriptions and examples, or move shared definitions into fewer $defs"
      }
    );
  }
}

function readSchema(appDir: string, event: string, path: string): Record<string, unknown> {
  let text: string;
  try {
    text = readFileSync(resolve(appDir, path), "utf8");
  } catch (cause) {
    const code = (cause as NodeJS.ErrnoException).code;
    throw new CliError(
      `event \`${event}\`: its schema file ${path} could not be read${code ? ` (${code})` : ""}`,
      {
        code: ExitCode.FAILURE,
        hint: `events.${event}.schema in oxy-app.json is a path relative to the app directory`
      }
    );
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    throw new CliError(
      `event \`${event}\`: its schema file ${path} is not valid JSON — ${(cause as Error).message}`,
      { code: ExitCode.FAILURE, hint: "the file must hold one JSON Schema (draft 2020-12) object" }
    );
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new CliError(`event \`${event}\`: its schema file ${path} is not a JSON object`, {
      code: ExitCode.FAILURE,
      hint: "the file must hold one JSON Schema (draft 2020-12) object"
    });
  }
  return parsed as Record<string, unknown>;
}

/**
 * Write the embedded schemas into the BUNDLE's `oxy-app.json`, the copy the
 * platform reads. The source manifest is never touched. A bundle without a
 * manifest, or whose manifest declares no events, is left alone. Returns how
 * many schemas were embedded.
 */
export function embedEventSchemasIntoBundle(appDir: string, bundleDir: string): number {
  const path = join(bundleDir, "oxy-app.json");
  if (!existsSync(path)) return 0;
  let bundled: OxyAppManifest;
  try {
    bundled = JSON.parse(readFileSync(path, "utf8")) as OxyAppManifest;
  } catch {
    return 0;
  }
  const paths = Object.values(bundled.events ?? {}).filter(
    (declaration) => typeof declaration?.schema === "string"
  ).length;
  if (paths === 0) return 0;
  writeFileSync(path, `${JSON.stringify(embedEventSchemas(appDir, bundled), null, 2)}\n`);
  return paths;
}
