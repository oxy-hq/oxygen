// The two merge signals: `review-gate` (the automated review's verdict) and
// `high-risk-approval` (a person signed off on the short list of risky paths).

import type { RiskArea } from "../config.ts";
import type { OwnerRule } from "./codeowners.ts";
import { ownersFor } from "./codeowners.ts";
import { matchesAny } from "./globs.ts";
import type { CommitStatus, PrReview, ReviewState } from "./types.ts";

const cap = (s: string): string => s.slice(0, 140); // GitHub truncates status descriptions

export interface GateInput {
  state: ReviewState | null;
  headSha: string;
  labels: readonly string[];
  draft: boolean;
  overrideLabel: string;
  reviewJobFailed?: boolean;
  /**
   * Set for the release app's own release PR (RELEASE_AUTOMATION): "bump" when its diff
   * is the pure version bump, "other" when it is not. claude-review.yaml does not run for
   * the app's pushes, so "other" with no review of the head must not read as waiting.
   */
  release?: "bump" | "other" | undefined;
}

/**
 * `review-gate` for the PR's current head. `reviewJobFailed` is set when the review
 * workflow for this head ended in failure: with no state for the head, that means it
 * died before posting, which must not read as "still waiting".
 */
export function evaluateGate({
  state,
  headSha,
  labels,
  draft,
  overrideLabel,
  reviewJobFailed = false,
  release
}: GateInput): CommitStatus {
  if (draft) return { state: "success", description: "Draft: not gated" };
  if (release === "bump")
    return { state: "success", description: "Not reviewed: a version bump the release app opened" };
  const overridden = labels.includes(overrideLabel);
  if (!state || state.head !== headSha) {
    if (reviewJobFailed && !overridden)
      return {
        state: "failure",
        description: "The review job failed before it posted a verdict. Re-run it"
      };
    if (release === "other" && !overridden)
      return {
        state: "failure",
        description:
          "Not a pure version bump, so it needs a review. If none is running, close and reopen the PR"
      };
    return overridden
      ? {
          state: "success",
          description: cap(`Overridden by ${overrideLabel}; this push has no finished review`)
        }
      : { state: "pending", description: cap(`Waiting for the review of ${headSha.slice(0, 9)}`) };
  }
  if (state.status === "incomplete") {
    return overridden
      ? {
          state: "success",
          description: cap(`Overridden by ${overrideLabel}; the review did not finish`)
        }
      : {
          state: "failure",
          description:
            "The review did not finish. Re-run it, or a maintainer applies review-override"
        };
  }
  if (state.verdict === "Blocking") {
    const ids = state.findings
      .filter((f) => f.status === "open" && f.severity === "blocking")
      .map((f) => f.id);
    return overridden
      ? {
          state: "success",
          description: cap(`Overridden by ${overrideLabel} (blocking: ${ids.join(", ")})`)
        }
      : {
          state: "failure",
          description: cap(
            `Blocking: ${ids.join(", ")}. Fix, dispute in the thread, or review-override`
          )
        };
  }
  if (state.status === "skipped")
    return { state: "success", description: cap(`Not reviewed: ${state.reason ?? "skipped"}`) };
  return {
    state: "success",
    description: cap(
      `${state.verdict ?? "LGTM"}${state.status === "partial" ? " (partial review)" : ""}`
    )
  };
}

/** The latest review state per reviewer, as GitHub counts approvals. */
export function latestReviews(reviews: readonly PrReview[]): Map<string, string> {
  const latest = new Map<string, string>();
  for (const r of reviews) {
    if (!r.user || r.state === "COMMENTED" || r.state === "PENDING") continue;
    latest.set(r.user.login, r.state);
  }
  return latest;
}

export interface RiskInput {
  files: readonly string[];
  reviews: readonly PrReview[];
  author: string;
  rules: readonly OwnerRule[];
  highRisk: readonly RiskArea[];
}

/** A HIGH_RISK area a PR touches: why it needs a person, and the files that put it there. */
export interface TouchedArea {
  id: string;
  reason: string;
  files: string[];
}

/** One touched area as the gate judged it. No `owners` means anyone but the author. */
export interface AreaVerdict extends TouchedArea {
  owners: string[];
  approvedBy: string | null;
}

/** The HIGH_RISK areas `files` touch, in config order, each with its reason. */
export function touchedAreas(
  files: readonly string[],
  highRisk: readonly RiskArea[]
): TouchedArea[] {
  const unique = [...new Set(files)];
  return highRisk.flatMap((area) => {
    const hit = unique.filter((path) => matchesAny(path, area.paths));
    return hit.length ? [{ id: area.id, reason: area.reason, files: hit }] : [];
  });
}

const whoCan = (a: AreaVerdict): string =>
  a.owners.length ? a.owners.join(" or ") : "anyone but the author";

/** `high-risk-approval`: a non-author approval from an owner of a risky path touched. */
export function evaluateRisk({
  files,
  reviews,
  author,
  rules,
  highRisk
}: RiskInput): CommitStatus & { areas: AreaVerdict[] } {
  const approvers = [...latestReviews(reviews)]
    .filter(([who, s]) => s === "APPROVED" && who !== author)
    .map(([who]) => who);
  const areas = touchedAreas(files, highRisk).map((area): AreaVerdict => {
    // Teams (`org/team`) are not resolved to members, so only named people count.
    const owners = [
      ...new Set(
        area.files.flatMap((path) => ownersFor(path, rules)).filter((o) => !o.includes("/"))
      )
    ];
    const approvedBy = approvers.find((who) => !owners.length || owners.includes(who)) ?? null;
    return { ...area, owners, approvedBy };
  });
  if (!areas.length)
    return { state: "success", description: "No high-risk paths touched", areas: [] };
  const ids = areas.map((a) => a.id).join(", ");
  const missing = areas.filter((a) => !a.approvedBy);
  if (!missing.length) return { state: "success", description: cap(`Approved for ${ids}`), areas };
  // With each reason when that fits the description; without, when it does not.
  const needs = (why: boolean) =>
    `Needs an approval: ${missing.map((a) => `${a.id}${why ? `, ${a.reason}` : ""} (${whoCan(a)})`).join("; ")}`;
  const long = needs(true);
  return { state: "failure", description: cap(long.length <= 140 ? long : needs(false)), areas };
}

const MAX_FILES = 5;

/** Up to MAX_FILES paths as code, then how many more. */
function fileList(files: readonly string[]): string {
  const shown = files.slice(0, MAX_FILES).map((f) => `\`${f}\``);
  const more = files.length - shown.length;
  return more > 0 ? `${shown.join(", ")} and ${more} more` : shown.join(", ");
}

/**
 * The gate run's step summary, behind the status's Details link: one line per area with
 * its reason, who approved or can, and its files. Owners carry no `@`, so a re-run pings
 * nobody.
 */
export function riskReport(areas: readonly AreaVerdict[]): string {
  const line = (a: AreaVerdict) =>
    `- **${a.id}** (${a.reason}): ${a.approvedBy ? `approved by ${a.approvedBy}` : `needs ${whoCan(a)}`} · ${fileList(a.files)}`;
  return ["### high-risk-approval", "", ...areas.map(line), ""].join("\n");
}
