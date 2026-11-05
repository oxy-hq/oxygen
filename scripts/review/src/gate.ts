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

/** `high-risk-approval`: a non-author approval from an owner of a risky path touched. */
export function evaluateRisk({
  files,
  reviews,
  author,
  rules,
  highRisk
}: RiskInput): CommitStatus & { areas: string[] } {
  const areas = new Map<string, Set<string>>();
  for (const path of files) {
    for (const area of highRisk) {
      if (!matchesAny(path, area.paths)) continue;
      const owners = areas.get(area.id) ?? new Set<string>();
      for (const o of ownersFor(path, rules)) if (!o.includes("/")) owners.add(o);
      areas.set(area.id, owners);
    }
  }
  if (!areas.size)
    return { state: "success", description: "No high-risk paths touched", areas: [] };
  const approvers = [...latestReviews(reviews)]
    .filter(([who, s]) => s === "APPROVED" && who !== author)
    .map(([who]) => who);
  const missing: string[] = [];
  for (const [id, owners] of areas) {
    const ok = owners.size ? approvers.some((a) => owners.has(a)) : approvers.length > 0;
    if (!ok)
      missing.push(
        owners.size ? `${id} (${[...owners].join(" or ")})` : `${id} (anyone but the author)`
      );
  }
  const ids = [...areas.keys()];
  if (!missing.length)
    return { state: "success", description: cap(`Approved for ${ids.join(", ")}`), areas: ids };
  return {
    state: "failure",
    description: cap(`Needs an approval: ${missing.join("; ")}`),
    areas: ids
  };
}
