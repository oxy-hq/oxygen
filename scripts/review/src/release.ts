// Release PRs that need no review: see RELEASE_AUTOMATION in config.ts.

import type { ReleaseRule } from "../config.ts";
import { parseUnifiedDiff } from "./diff.ts";
import type { DiffFile, PrFileListing, PrMeta } from "./types.ts";

const VERSION_LINE = /^\s*version\s*=\s*"[^"]*"\s*$/;

/** The release app opened this PR from a release branch, whatever it now contains. */
export function isReleaseAppPr(meta: PrMeta, rule: ReleaseRule): boolean {
  const author = meta.user;
  if (author.type !== "Bot" || !rule.authors.includes(author.login.replace(/\[bot\]$/, "")))
    return false;
  const ref = meta.head.ref ?? "";
  return rule.prefixes.some((p) => ref.startsWith(p));
}

/** True only for a version bump the release app opened; content decides, not names. */
export function isReleaseBump(
  { meta, files }: { meta: PrMeta; files: readonly DiffFile[] },
  rule: ReleaseRule
): boolean {
  if (!isReleaseAppPr(meta, rule)) return false;
  if (!files.length) return false;
  for (const f of files) {
    if (!rule.files.includes(f.path) || f.status !== "modified") return false;
    if (!rule.versionOnly.includes(f.path)) continue;
    for (const h of f.hunks)
      for (const l of h.lines) if (l.type !== " " && !VERSION_LINE.test(l.text)) return false;
  }
  return true;
}

/**
 * The listing as diff files, for the gate, which reads the PR only through the API. Null
 * when the listing cannot prove a bump: a file that is not plainly modified, or one the
 * API sent no patch for, since a missing patch would read as a file with no changed lines.
 */
export function listingAsDiff(listed: readonly PrFileListing[]): DiffFile[] | null {
  const files: DiffFile[] = [];
  for (const f of listed) {
    if (f.status !== "modified" || f.patch === undefined) return null;
    files.push(...parseUnifiedDiff(`diff --git a/${f.filename} b/${f.filename}\n${f.patch}\n`));
  }
  return files;
}
