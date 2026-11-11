// This package is public; the repository it is built from is not.
//
// `files` ships `dist`, `sourcemap` is on and `minify` is off, so a comment in
// `src/**` travels three ways: verbatim in the bundle, verbatim again inside
// the sourcemap's `sourcesContent`, and into any JSON Schema generated from a
// doc comment. Twenty references to private paths had accumulated that way
// before anyone looked — including one that reached a reader, a `docs:` field
// on an error report pointing at a file only Oxy can open.
//
// Keeping `sourcesContent` is deliberate: this is a library people debug INTO
// from their own apps, so a stack frame that resolves to readable SDK source is
// a feature. The cost of keeping it is this test.
//
// Test files are exempt: they are not in any entry point's import graph, so
// they reach neither the bundle nor the map.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = join(import.meta.dirname, ".");

/** Patterns that name something outside this package. */
const FORBIDDEN: Array<{ what: string; re: RegExp }> = [
  { what: "a Rust source path", re: /crates\/[a-z0-9/_-]+\.rs/g },
  { what: "an internal-docs file", re: /internal-docs\/[a-z0-9.-]+\.(?:md|mdx)/g },
  // Internal authority identifiers: `Ring::`, `Cap::`, `Action::`. They mean
  // nothing without the source that defines them.
  { what: "an internal authority name", re: /\b(?:Ring|Cap|Action)::[A-Za-z]+/g }
];

function sourceFiles(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      sourceFiles(path, found);
      continue;
    }
    if (!/\.(?:ts|tsx)$/.test(entry)) continue;
    // Tests ship nowhere, and fixtures in them legitimately name real things.
    if (/\.test\.(?:ts|tsx)$/.test(entry)) continue;
    found.push(path);
  }
  return found;
}

describe("published source", () => {
  it("names nothing a reader outside this repository cannot open", () => {
    const offences: string[] = [];
    for (const path of sourceFiles(SRC)) {
      const text = readFileSync(path, "utf8");
      for (const { what, re } of FORBIDDEN) {
        for (const hit of text.match(re) ?? []) {
          const line = text.slice(0, text.indexOf(hit)).split("\n").length;
          const rel = path.slice(SRC.length + 1);
          offences.push(`${rel}:${line} names ${what}: ${hit}`);
        }
      }
    }
    // State the behaviour instead. The reader has the package, not the source.
    expect(offences).toEqual([]);
  });

  it("scans a meaningful number of files, so a broken walk cannot pass", () => {
    expect(sourceFiles(SRC).length).toBeGreaterThan(40);
  });
});
