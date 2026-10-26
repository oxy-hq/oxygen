// The `roadmap` merge status: a PR that changes code names the roadmap line it starts,
// moves or lands, and that line cites the PR; or it says `Roadmap: none -- <why>`.
// internal-docs/platform-roadmap.md (Governance) is the rule; this is the check.

import {
  allItems,
  findItem,
  parseRoadmap,
  pinnedId,
  renderSummary,
  sameLines
} from "../../roadmap/doc.ts";
import type { Trailer } from "../../roadmap/trailer.ts";
import type { RoadmapConfig } from "../config.ts";
import { matchesAny } from "./globs.ts";
import type { CommitStatus } from "./types.ts";

const cap = (s: string): string => s.slice(0, 140); // GitHub truncates status descriptions

export interface RoadmapInput {
  /** Every path the PR touches (renames: both names). */
  files: readonly string[];
  trailer: Trailer;
  /** The PR number, when known: a named line must cite it. */
  pr: number | null;
  draft: boolean;
  /** The roadmap doc and the summary at the PR head; `null` when the file is absent. */
  doc: string | null;
  summary: string | null;
  /** The roadmap doc at the merge base, when the PR changes it: tells a pin or a rewrap from a move. */
  docBefore?: string | null;
  config: RoadmapConfig;
}

/** The product paths the PR touches, exemptions removed. */
export function productFiles(files: readonly string[], config: RoadmapConfig): string[] {
  return files.filter(
    (f) => matchesAny(f, config.productPaths) && !matchesAny(f, config.exemptPaths)
  );
}

const HOW = "Roadmap: <line-id> or Roadmap: none -- <why>";

export function evaluateRoadmap({
  files,
  trailer,
  pr,
  draft,
  doc,
  summary,
  docBefore = null,
  config
}: RoadmapInput): CommitStatus {
  if (draft) return { state: "success", description: "Draft: not gated" };
  const product = productFiles(files, config);
  const docChanged = files.includes(config.doc);
  const summaryChanged = files.includes(config.summary);

  if (trailer.kind === "invalid")
    return { state: "failure", description: cap(`Roadmap trailer ${trailer.problem}`) };

  if ((docChanged || summaryChanged) && !doc)
    return { state: "failure", description: cap(`${config.doc} is missing at the PR head`) };
  const parsed = doc ? parseRoadmap(doc) : null;
  // A changed roadmap keeps its one-page summary derived, whatever the PR says about itself.
  if ((docChanged || summaryChanged) && doc && summary !== renderSummary(doc))
    return {
      state: "failure",
      description: cap(
        "The one-page summary is not derived from the roadmap: node scripts/roadmap/roadmap.ts summary --write"
      )
    };
  // Every line carries its pin: the pin is the one id the gate, the doc and the roadmap
  // app agree on, so a line without one has no id anyone can name.
  const unpinned = parsed && docChanged ? allItems(parsed).filter((i) => !pinnedId(i.text)) : [];
  if (unpinned.length)
    return {
      state: "failure",
      description: cap(
        `${unpinned.length} roadmap line(s) have no <!-- rm:id --> pin: node scripts/roadmap/roadmap.ts pin --write`
      )
    };
  // Pins, rewrapping, governance prose or a scoreboard reading move no line; a PR that
  // only does that needs no line id.
  const moved = docChanged && !(parsed && docBefore && sameLines(parseRoadmap(docBefore), parsed));

  if (trailer.kind === "missing") {
    if (moved)
      return {
        state: "failure",
        description: cap(`The PR moves a roadmap line: name it. ${HOW}`)
      };
    if (product.length)
      return {
        state: "failure",
        description: cap(
          `${product.length} product file(s) changed with no roadmap trailer. ${HOW}`
        )
      };
    return {
      state: "success",
      description: "No product code changed; no roadmap trailer needed"
    };
  }

  if (trailer.kind === "none") {
    if (moved)
      return {
        state: "failure",
        description: cap(
          "The PR moves a roadmap line but says `Roadmap: none`: name the line it changes"
        )
      };
    return { state: "success", description: cap(`Roadmap: none (${trailer.reason})`) };
  }

  // Named lines: each must exist at the head and cite this PR.
  if (!parsed)
    return { state: "failure", description: cap(`${config.doc} is missing at the PR head`) };
  const unknown = trailer.ids.filter((id) => !findItem(parsed, id));
  if (unknown.length)
    return {
      state: "failure",
      description: cap(
        `No roadmap line ${unknown.join(", ")} at the PR head. Ids: node scripts/roadmap/roadmap.ts ids`
      )
    };
  if (pr !== null) {
    const silent = trailer.ids.filter((id) => !findItem(parsed, id)?.prs.includes(pr));
    if (silent.length)
      return {
        state: "failure",
        description: cap(
          `Line ${silent.join(", ")} does not cite #${pr}: node scripts/roadmap/roadmap.ts link --id <id> --pr ${pr} --write`
        )
      };
  }
  return { state: "success", description: cap(`Roadmap: ${trailer.ids.join(", ")}`) };
}
