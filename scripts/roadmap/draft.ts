// `roadmap.ts draft`: the brief an agent (or a person) reads before writing a PR's
// `Roadmap:` trailer. Deterministic: the status's own verdict for the branch as it
// stands, the product files it counts, and the roadmap lines whose words the branch
// shares, so the choice is made from the doc's own ids, not from memory.

import type { CommitStatus } from "../review/src/types.ts";
import type { Doc, Item } from "./doc.ts";
import { allItems, headline } from "./doc.ts";

const STOP = new Set(
  (
    "with that this from into when then than have file files feat fixes refactor chore " +
    "docs test tests build style perf src crates web-app sdk app core main lib mod index " +
    "their there these those which while where about after before also only over under " +
    "each every some such more most other another every every into onto between through " +
    "landed line lines done roadmap oxygen oxy review json yaml hook hooks skill skills " +
    "status trailer check script scripts internal claude"
  ).split(/\s+/)
);

/** The words a text offers for matching: lower-case, four letters or more, no stopwords. */
export function words(text: string): Set<string> {
  const out = new Set<string>();
  for (const w of text.toLowerCase().match(/[a-z][a-z-]{3,}/g) ?? []) {
    for (const part of w.split("-")) if (part.length >= 4 && !STOP.has(part)) out.add(part);
  }
  return out;
}

export interface Ranked {
  item: Item;
  shared: string[];
}

/**
 * Roadmap lines ordered by the words they share with the branch (its title, commit
 * subjects and changed paths). Landed lines rank last at equal score: a new PR seldom
 * belongs on one. Lines sharing nothing are left out.
 */
export function rankLines(doc: Doc, signals: readonly string[]): Ranked[] {
  const have = words(signals.join(" "));
  const ranked = allItems(doc)
    .map((item) => ({ item, shared: [...words(item.text)].filter((w) => have.has(w)).sort() }))
    .filter((r) => r.shared.length > 0);
  ranked.sort(
    (a, b) => b.shared.length - a.shared.length || Number(a.item.landed) - Number(b.item.landed)
  );
  return ranked;
}

export interface DraftInput {
  title: string;
  files: readonly string[];
  product: readonly string[];
  verdict: CommitStatus;
  ranked: readonly Ranked[];
  max?: number;
}

/** The brief, as `roadmap.ts draft` prints it. */
export function renderDraft({
  title,
  files,
  product,
  verdict,
  ranked,
  max = 6
}: DraftInput): string {
  const out: string[] = [];
  out.push(`Title: ${title}`);
  out.push(
    product.length
      ? `Product files (${product.length} of ${files.length} changed): ${product.join(", ")}`
      : `Product files: none of the ${files.length} changed files count (tests, docs, config and scaffolds are exempt)`
  );
  out.push(`With the body as given, the status says: ${verdict.state} · ${verdict.description}`);
  out.push("");
  if (ranked.length) {
    out.push("Roadmap lines sharing words with this branch (closest first):");
    for (const r of ranked.slice(0, max)) {
      const where = `${r.item.horizon}${r.item.landed ? ", landed" : ""}`;
      out.push(`  ${r.item.id}  (${where})  ${headline(r.item.text)}  [${r.shared.join(", ")}]`);
    }
  } else out.push("No roadmap line shares a word with this branch.");
  out.push("All ids: node scripts/roadmap/roadmap.ts ids");
  out.push("");
  out.push("Pick one row of the oxy-roadmap skill's table, then end the PR body with the trailer:");
  out.push(
    "  lands, starts or contributes to a line  Roadmap: <id>   and edit the line; after the PR opens:"
  );
  out.push(
    "                                          node scripts/roadmap/roadmap.ts link --id <id> --pr <n> --write"
  );
  out.push(
    "  behaviour no line names                 add a line under Now, then pin --write, summary --write, Roadmap: <new-id>"
  );
  out.push("  no product behaviour changes            Roadmap: none -- <why>");
  return out.join("\n");
}
