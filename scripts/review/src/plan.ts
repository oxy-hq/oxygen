// What a review covers: which files, which focused lenses, which guidance to read.

import type * as Config from "../config.ts";
import { addedText } from "./diff.ts";
import { matchesAny } from "./globs.ts";
import type { DiffFile, Lens, LensPick, Plan } from "./types.ts";

type PlanConfig = Pick<
  typeof Config,
  "IGNORE" | "LENSES" | "MAX_LENSES" | "PATCH_EXCLUDE" | "LARGE_PR_LINES"
>;

function lensTrigger(
  lens: Lens,
  files: readonly DiffFile[],
  patchExclude: readonly string[] = []
): string | null {
  for (const f of files) {
    if (lens.paths.length && matchesAny(f.path, lens.paths)) return `touches \`${f.path}\``;
  }
  if (!lens.patch) return null;
  for (const f of files) {
    if (lens.patchPaths && !matchesAny(f.path, lens.patchPaths)) continue;
    if (matchesAny(f.path, patchExclude)) continue;
    const m = addedText(f).match(lens.patch);
    if (m) return `adds \`${m[0].trim().slice(0, 40)}\` in \`${f.path}\``;
  }
  return null;
}

/**
 * What an incremental round matches lenses against. `delta` is the diff since the last
 * reviewed head, limited to the PR's files; `covered` are the lenses that finished on an
 * earlier full review of this PR (`lensesFinished` in load.ts).
 */
export interface Since {
  delta: readonly DiffFile[];
  covered: ReadonlySet<string>;
}

/**
 * Why a lens runs this round, or null. On an incremental round a covered lens is
 * triggered by the new changes only: the whole-PR diff of a touched file still holds every
 * line the full review's lens already read, so matching it re-ran the lens on every push.
 * A lens not covered (dropped for the cap, skipped for budget, failed, or not triggered
 * until a later push) matches each touched file's whole-PR diff, as every push did before.
 */
function triggerFor(
  lens: Lens,
  files: readonly DiffFile[],
  patchExclude: readonly string[],
  since: Since | null
): string | null {
  if (!since) return lensTrigger(lens, files, patchExclude);
  const now = lensTrigger(lens, since.delta, patchExclude);
  if (now || since.covered.has(lens.id)) return now;
  const earlier = lensTrigger(lens, files, patchExclude);
  return earlier && `${earlier} (no full review of this PR has run this lens)`;
}

/** Lenses the diff triggers, in priority order, capped at `max`. */
export function selectLenses(
  files: readonly DiffFile[],
  lenses: readonly Lens[],
  max: number,
  patchExclude: readonly string[] = [],
  since: Since | null = null
): { selected: LensPick[]; dropped: LensPick[] } {
  const triggered: LensPick[] = [];
  for (const lens of lenses) {
    const reason = triggerFor(lens, files, patchExclude, since);
    if (reason) triggered.push({ id: lens.id, prompt: lens.prompt, reason });
  }
  return { selected: triggered.slice(0, max), dropped: triggered.slice(max) };
}

/**
 * The nearest CLAUDE.md above each file, excluding the root one (already loaded).
 * `exists` is injected so this stays pure.
 */
export function guidanceFor(paths: readonly string[], exists: (path: string) => boolean): string[] {
  const found = new Set<string>();
  for (const p of paths) {
    const parts = p.split("/").slice(0, -1);
    for (let i = parts.length; i > 0; i--) {
      const candidate = `${parts.slice(0, i).join("/")}/CLAUDE.md`;
      if (exists(candidate)) {
        found.add(candidate);
        break;
      }
    }
  }
  return [...found].sort((a, b) => a.localeCompare(b));
}

/**
 * Build the plan. `changedSince` is the set of PR files that changed since the last
 * reviewed head (null for a full review). `delta` is that change as a diff and `covered`
 * the lenses that finished on an earlier round; an incremental round given both matches
 * lenses against the delta (see `triggerFor`).
 */
export function buildPlan({
  files,
  config,
  exists,
  changedSince = null,
  delta = null,
  covered = new Set<string>()
}: {
  files: readonly DiffFile[];
  config: PlanConfig;
  exists: (path: string) => boolean;
  changedSince?: Set<string> | null;
  delta?: readonly DiffFile[] | null;
  covered?: ReadonlySet<string>;
}): Plan {
  const reviewable: DiffFile[] = [];
  const ignored: string[] = [];
  for (const f of files) {
    if (f.binary || matchesAny(f.path, config.IGNORE)) ignored.push(f.path);
    else reviewable.push(f);
  }
  const inScope = changedSince ? reviewable.filter((f) => changedSince.has(f.path)) : reviewable;
  const inScopePaths = new Set(inScope.map((f) => f.path));
  const since =
    changedSince && delta
      ? { delta: delta.filter((f) => inScopePaths.has(f.path)), covered }
      : null;
  const { selected, dropped } = selectLenses(
    inScope,
    config.LENSES,
    config.MAX_LENSES,
    config.PATCH_EXCLUDE,
    since
  );
  const lines = reviewable.reduce((n, f) => n + f.additions + f.deletions, 0);
  return {
    mode: changedSince ? "incremental" : "full",
    reviewable: reviewable.map(({ path, status, additions, deletions }) => ({
      path,
      status,
      additions,
      deletions
    })),
    inScope: inScope.map((f) => f.path),
    ignored,
    lenses: selected,
    droppedLenses: dropped.map((l) => ({
      id: l.id,
      reason: `more than ${config.MAX_LENSES} lenses triggered`
    })),
    guidance: guidanceFor(
      inScope.map((f) => f.path),
      exists
    ),
    size: { files: reviewable.length, lines },
    large: lines > config.LARGE_PR_LINES
  };
}
