/**
 * Is the app building against a kit `oxyc` knows to be old?
 *
 * The manifest contract, the function runtime's types and the Vite plugin's
 * build move together; an app pinned to an SDK several minors behind the one
 * this `oxyc` was built beside validates against a contract the server has
 * outgrown, and finds out at the first call. This says so at publish — ONE
 * line per package, a warning, never a refusal: the publish is the author's
 * call, and a pin is often deliberate.
 *
 * NO NETWORK. The versions compared against are the sibling packages' at the
 * time `oxyc` was built (`scripts/emit-version.mjs` bakes them in), and the
 * app's are read from its `package.json` — the installed copy under
 * `node_modules` when there is one, else the declared range's floor.
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { KIT_VERSIONS } from "../generated/version.js";

/** `major.minor.patch` of a version or a range, from its first dotted triple. */
export function versionFloor(spec: string): [number, number, number] | undefined {
  const match = /(\d+)\.(\d+)\.(\d+)/.exec(spec);
  if (!match) return undefined;
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

/** More than one minor behind, or any major behind. */
export function isStale(app: [number, number, number], known: [number, number, number]): boolean {
  if (app[0] !== known[0]) return app[0] < known[0];
  return known[1] - app[1] > 1;
}

function readJson(path: string): Record<string, unknown> | undefined {
  try {
    const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
    return typeof parsed === "object" && parsed !== null
      ? (parsed as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
}

/** The app's version of `name`: installed if present, else the declared range. */
function appVersion(
  appDir: string,
  name: string
): { spec: string; installed: boolean } | undefined {
  const installed = join(appDir, "node_modules", ...name.split("/"), "package.json");
  if (existsSync(installed)) {
    const version = readJson(installed)?.version;
    if (typeof version === "string") return { spec: version, installed: true };
  }
  const pkg = readJson(join(appDir, "package.json"));
  for (const field of ["dependencies", "devDependencies"]) {
    const deps = pkg?.[field];
    if (typeof deps === "object" && deps !== null) {
      const spec = (deps as Record<string, unknown>)[name];
      if (typeof spec === "string") return { spec, installed: false };
    }
  }
  return undefined;
}

/**
 * One warning per kit package the app uses that is more than a minor behind
 * the version this `oxyc` was built with. Empty when the app names none, or
 * when the versions were not baked in (a build outside the repo).
 */
export function staleKitWarnings(
  appDir: string,
  known: Record<string, string | null> = KIT_VERSIONS
): string[] {
  const out: string[] = [];
  for (const [name, knownSpec] of Object.entries(known)) {
    if (knownSpec === null) continue;
    const knownFloor = versionFloor(knownSpec);
    const app = appVersion(appDir, name);
    const appFloor = app ? versionFloor(app.spec) : undefined;
    if (!knownFloor || !app || !appFloor || !isStale(appFloor, knownFloor)) continue;
    const have = app.installed ? app.spec : `${app.spec} (declared; not installed here)`;
    out.push(
      `${name} ${have} is behind the ${knownSpec} this oxyc was built with — ` +
        `\`pnpm add ${name}@latest\` so the app validates and runs against the current contract`
    );
  }
  return out;
}

/** The SDK an app's design-system check is held to. */
export const SDK_PACKAGE = "@oxy-hq/sdk";

/**
 * The `@oxy-hq/sdk` version the app builds against, for `oxyc publish`'s
 * `sdk_version` field: the server stores it on the build, and the design-system
 * check holds it to the SDK floor. The installed copy's own version when there
 * is one, else the floor of the declared range (`^2.30.0` → `2.30.0`).
 * Undefined when the app does not depend on the SDK, or declares it in a form
 * with no version in it (`workspace:*` with nothing installed): the field is
 * then left out, which is not the same as claiming a version.
 */
export function sdkVersion(appDir: string): string | undefined {
  const found = appVersion(appDir, SDK_PACKAGE);
  if (!found) return undefined;
  if (found.installed) return found.spec;
  return versionFloor(found.spec)?.join(".");
}
