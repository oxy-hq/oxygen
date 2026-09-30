// Which screens render a changed file: walk the web-app's imports backwards
// from each changed module to the page components App.tsx mounts. The planner
// gets these as grounded candidates instead of guessing where a shared
// component shows up.
//
// Regex, not a parser: TypeScript 7 ships no JS compiler API, and an import
// graph needs only specifiers. A specifier the patterns miss costs a candidate,
// never a wrong one.

import { dirname, join, normalize } from "node:path";

export type ReadSource = (path: string) => string | undefined;

export interface EntryHit {
  /** The identifier App.tsx binds the page to, e.g. `ThreadPage`. */
  entry: string;
  /** The page module. */
  file: string;
  /** Import chain from the page down to the changed file. */
  via: string[];
}

const SRC = "web-app/src";
const APP = `${SRC}/App.tsx`;
const EXTENSIONS = ["", ".ts", ".tsx", "/index.ts", "/index.tsx"];
const STATIC_IMPORT = /(?:import|export)\s[^'"`;]*?from\s*["']([^"']+)["']/g;
const SIDE_EFFECT_IMPORT = /import\s*["']([^"']+)["']/g;
const DYNAMIC_IMPORT = /import\s*\(\s*["']([^"']+)["']\s*\)/g;

export function importSpecifiers(source: string): string[] {
  const out = new Set<string>();
  for (const re of [STATIC_IMPORT, SIDE_EFFECT_IMPORT, DYNAMIC_IMPORT]) {
    for (const m of source.matchAll(re)) out.add(m[1]);
  }
  return [...out];
}

/** Resolve a specifier to a known source file, or undefined for packages. */
export function resolveSpecifier(
  from: string,
  spec: string,
  known: Set<string>
): string | undefined {
  let base: string;
  if (spec.startsWith("@/")) base = join(SRC, spec.slice(2));
  else if (spec.startsWith(".")) base = normalize(join(dirname(from), spec));
  else return undefined;
  for (const ext of EXTENSIONS) {
    if (known.has(base + ext)) return base + ext;
  }
  return undefined;
}

/** importer → importee edges, reversed: for each file, who imports it. */
export function reverseGraph(files: string[], read: ReadSource): Map<string, Set<string>> {
  const known = new Set(files);
  const importers = new Map<string, Set<string>>();
  for (const file of files) {
    const source = read(file);
    if (source === undefined) continue;
    for (const spec of importSpecifiers(source)) {
      const target = resolveSpecifier(file, spec, known);
      if (!target) continue;
      if (!importers.has(target)) importers.set(target, new Set());
      importers.get(target)?.add(file);
    }
  }
  return importers;
}

/** Page modules App.tsx mounts, keyed by file, valued by the identifier it binds. */
export function pageEntries(appSource: string, known: Set<string>): Map<string, string> {
  const entries = new Map<string, string>();
  const staticRe = /import\s+(\w+)\s+from\s*["']([^"']+)["']/g;
  const lazyRe =
    /const\s+(\w+)\s*=\s*React\.lazy\(\s*\(\)\s*=>\s*import\(\s*["']([^"']+)["']\s*\)/g;
  for (const re of [staticRe, lazyRe]) {
    for (const m of appSource.matchAll(re)) {
      const file = resolveSpecifier(APP, m[2], known);
      if (file?.includes("/pages/")) entries.set(file, m[1]);
    }
  }
  return entries;
}

/**
 * Breadth-first from each changed file up the importer edges, stopping at the
 * first page entry on each path. Shortest chains first; at most `limit`.
 */
export function entriesReaching(
  changed: string[],
  importers: Map<string, Set<string>>,
  entries: Map<string, string>,
  limit = 8
): EntryHit[] {
  const hits = new Map<string, EntryHit>();
  for (const start of changed) {
    const seen = new Set([start]);
    let frontier: string[][] = [[start]];
    while (frontier.length > 0) {
      const next: string[][] = [];
      for (const chain of frontier) {
        const head = chain[0];
        const entry = entries.get(head);
        if (entry) {
          const prev = hits.get(head);
          if (!prev || prev.via.length > chain.length)
            hits.set(head, { entry, file: head, via: chain });
          continue;
        }
        for (const importer of importers.get(head) ?? []) {
          if (seen.has(importer)) continue;
          seen.add(importer);
          next.push([importer, ...chain]);
        }
      }
      frontier = next;
    }
  }
  return [...hits.values()].sort((a, b) => a.via.length - b.via.length).slice(0, limit);
}

export const APP_FILE = APP;

/**
 * App.tsx reduced to its routing: the lines that open or close a route tree,
 * name a path, or mount an element — indentation kept, so nesting reads. A
 * fifth of the file, and the part the planner needs to turn a page into a URL.
 */
export function routeTable(appSource: string): string {
  return appSource
    .split("\n")
    .filter((l) => /<\/?Routes?\b|\bpath[=:]|\belement[=:]|\bindex\b/.test(l))
    .map((l) => l.trimEnd())
    .join("\n");
}
