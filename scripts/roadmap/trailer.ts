// The `Roadmap:` trailer every PR body carries (internal-docs/platform-roadmap.md,
// Governance). Either the lines the PR starts, moves or lands:
//
//     Roadmap: promoting-and-rolling-back-a-build, one-status-word-for-is-this
//
// or a declaration that it changes no product behaviour, with the reason:
//
//     Roadmap: none -- dependency bump
//
// Several `Roadmap:` lines add up. The roadmap app reads the same trailer to link a PR
// to its line on the board.

import { isValidId } from "./doc.ts";

export type Trailer =
  | { kind: "missing" }
  | { kind: "none"; reason: string }
  | { kind: "lines"; ids: string[] }
  | { kind: "invalid"; problem: string };

// `[ \t]*`, not `\s*`: the value is on the trailer's own line, so an empty
// `Roadmap: ` (the template's) stays empty instead of reading the next line.
const TRAILER = /^[ \t]*Roadmap(?:-Item)?:[ \t]*(.*?)[ \t]*$/gim;
const NONE = /^none\b\s*(?:--|—|:|-)?\s*(.*)$/i;
const MIN_REASON = 8;

export function parseTrailer(body: string | null | undefined): Trailer {
  const ids: string[] = [];
  let none: { reason: string } | null = null;
  let found = false;
  for (const m of (body ?? "").matchAll(TRAILER)) {
    found = true;
    const value = (m[1] ?? "").replace(/`/g, "").trim();
    const n = NONE.exec(value);
    if (n) {
      none = { reason: (n[1] ?? "").trim() };
      continue;
    }
    for (const raw of value.split(/[\s,]+/)) {
      const id = raw.trim().toLowerCase();
      if (!id) continue;
      if (!isValidId(id))
        return { kind: "invalid", problem: `${JSON.stringify(raw)} is not a roadmap line id` };
      if (!ids.includes(id)) ids.push(id);
    }
  }
  if (!found) return { kind: "missing" };
  if (none && ids.length)
    return { kind: "invalid", problem: "says both `none` and a line id; pick one" };
  if (none) {
    if (none.reason.replace(/[^A-Za-z0-9]/g, "").length < MIN_REASON)
      return { kind: "invalid", problem: "`none` needs a reason: `Roadmap: none -- <why>`" };
    return { kind: "none", reason: none.reason };
  }
  if (!ids.length)
    return {
      kind: "invalid",
      problem: "is empty: `Roadmap: <line-id>` or `Roadmap: none -- <why>`"
    };
  return { kind: "lines", ids };
}
