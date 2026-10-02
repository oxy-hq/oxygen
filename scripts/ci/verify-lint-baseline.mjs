#!/usr/bin/env node
// Fail if `oxlint-suppressions.json` grew.
//
// The baseline records the type-aware lint violations that existed when each
// rule was turned on, per file and rule. Oxlint itself enforces the baseline:
// one violation more than recorded fails, and one fewer fails until the
// baseline is pruned. What it cannot enforce is the baseline's own size —
// when a new violation fails, its error says to run `oxlint --suppress-all`,
// and that command records the new violation as if it had always been there.
// This is the check that makes that a red build instead of a way out.
//
// Compared per RULE, not per file: a file that is moved or split carries its
// entries with it, and that is not growth.
//
//   node scripts/ci/verify-lint-baseline.mjs <base-ref>
//
// `<base-ref>` is the commit the change is measured against — the PR's base
// branch, or the previous commit on a push.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const BASELINE = "oxlint-suppressions.json";

/** Sum a suppressions file's counts by rule. */
export function totalsByRule(suppressions) {
  const totals = new Map();
  for (const rules of Object.values(suppressions)) {
    for (const [rule, { count }] of Object.entries(rules)) {
      totals.set(rule, (totals.get(rule) ?? 0) + count);
    }
  }
  return totals;
}

/** The rules whose total went up between two suppressions files. */
export function grown(base, head) {
  const before = totalsByRule(base);
  const after = totalsByRule(head);
  return [...after]
    .map(([rule, count]) => ({ rule, before: before.get(rule) ?? 0, after: count }))
    .filter((r) => r.after > r.before)
    .sort((a, b) => a.rule.localeCompare(b.rule));
}

function git(...args) {
  return execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

/** The baseline as of the point this change branched from `baseRef`, or null. */
function baselineAt(baseRef) {
  const mergeBase = git("merge-base", "HEAD", baseRef).trim();
  try {
    return JSON.parse(git("show", `${mergeBase}:${BASELINE}`));
  } catch {
    // The file did not exist there: this is the change that introduces it.
    return null;
  }
}

function main() {
  const baseRef = process.argv[2];
  if (!baseRef) {
    console.error("usage: verify-lint-baseline.mjs <base-ref>");
    return 2;
  }
  const head = JSON.parse(readFileSync(BASELINE, "utf8"));
  const base = baselineAt(baseRef);
  if (base === null) {
    console.log(`${BASELINE} is new in this change — nothing to compare against.`);
    return 0;
  }
  const growth = grown(base, head);
  if (growth.length === 0) {
    console.log(`${BASELINE} did not grow.`);
    return 0;
  }
  console.error(`${BASELINE} grew. A new violation was recorded instead of fixed:\n`);
  for (const { rule, before, after } of growth) {
    console.error(`  ${rule}: ${before} -> ${after}`);
  }
  console.error(
    "\nRestore the baseline (`git checkout <base> -- oxlint-suppressions.json`), run\n" +
      "`pnpm lint:bugs` to see the new violation, and fix it. If the code is right and\n" +
      "the rule is wrong there, say so at the site:\n" +
      "  // oxlint-disable-next-line typescript/<rule> -- <why>"
  );
  return 1;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exit(main());
}
