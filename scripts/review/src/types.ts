// The shapes that cross module boundaries. The review state is also a stored contract
// (base64 in the summary comment, read back by the next round and by the metrics), so
// changing a field here is a change to STATE_VERSION's meaning.

export const SEVERITIES = ["blocking", "should_fix", "nit"] as const;
export type Severity = (typeof SEVERITIES)[number];

export const CATEGORIES = [
  "correctness",
  "security",
  "authz",
  "tenant-isolation",
  "data-loss",
  "migration",
  "concurrency",
  "performance",
  "error-handling",
  "api-contract",
  "deploy-safety",
  "repo-rule",
  "test-gap",
  "pr-description",
  "maintainability",
  "style"
] as const;
export type Category = (typeof CATEGORIES)[number];

export type FindingStatus = "open" | "fixed" | "withdrawn" | "obsolete";
export type ClosedStatus = Exclude<FindingStatus, "open">;
export type ReviewStatus = "none" | "complete" | "partial" | "incomplete" | "skipped";
export type Verdict = "LGTM" | "LGTM with nits" | "Needs changes" | "Blocking" | "Not reviewed";
export type Mode = "full" | "incremental";

// ---- Diffs and plans ----------------------------------------------------------------

export interface DiffLine {
  type: "+" | "-" | " ";
  oldNo: number | null;
  newNo: number | null;
  text: string;
}

export interface Hunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export type FileStatus = "added" | "deleted" | "modified" | "renamed";

export interface DiffFile {
  path: string;
  oldPath: string;
  status: FileStatus;
  binary: boolean;
  additions: number;
  deletions: number;
  hunks: Hunk[];
}

export interface Lens {
  id: string;
  prompt: string;
  paths: readonly string[];
  patch?: RegExp;
  patchPaths?: readonly string[];
}

export interface LensPick {
  id: string;
  prompt: string;
  reason: string;
}

export interface Skipped {
  id: string;
  reason: string;
}

export interface PlannedFile {
  path: string;
  status: FileStatus;
  additions: number;
  deletions: number;
}

export interface Plan {
  mode: Mode;
  reviewable: PlannedFile[];
  inScope: string[];
  ignored: string[];
  lenses: LensPick[];
  droppedLenses: Skipped[];
  guidance: string[];
  size: { files: number; lines: number };
  large: boolean;
}

// ---- Findings -----------------------------------------------------------------------

/** One finding as a finder pass returns it (schemas.ts), before anything is trusted. */
export interface RawFinding {
  reasoning?: unknown;
  title?: unknown;
  file?: unknown;
  line?: unknown;
  end_line?: unknown;
  category?: unknown;
  severity?: unknown;
  confidence?: unknown;
  fix?: unknown;
  rule?: unknown;
}

/** A normalised finding, not yet given an id. */
export interface Candidate {
  title: string;
  file: string;
  line: number;
  end_line: number | null;
  category: Category;
  severity: Severity;
  confidence: number;
  reasoning: string;
  fix: string;
  rule: string | null;
  sources: string[];
  cid?: string;
  unverified?: boolean;
  why?: string;
}

/** A finding the PR carries, in the stored state. */
export interface Finding {
  id: string;
  title: string;
  file: string;
  line: number;
  end_line: number | null;
  severity: Severity;
  category: Category;
  confidence: number;
  lens: string;
  sources: string[];
  rule: string | null;
  status: FindingStatus;
  first: string;
  closed?: string;
  why?: string;
  reasoning?: string;
  fix?: string;
  inline?: number | null;
}

export interface Possible {
  title: string;
  file: string;
  line: number;
  severity: Severity;
  category: Category;
  why: string;
}

export interface PassSummary {
  ok: boolean;
  subtype: string;
  cost: number;
  ms: number;
  turns: number;
  tools: Record<string, number>;
}

export interface Round {
  sha: string;
  at: string;
  mode: string;
  status: ReviewStatus;
  verdict: Verdict | null;
  cost: number;
  ms: number;
  lines: number;
  lenses: string[];
  added: string[];
  closed: string[];
  candidates?: number;
  rejected?: number;
  filtered?: number;
  passes?: Record<string, PassSummary>;
}

/**
 * The general pass's answer to "does this PR change product behaviour?", kept for the
 * `roadmap` status. `line` is a pinned roadmap id, `"new"` when no line fits, or null.
 */
export interface RoadmapJudgement {
  behaviour: "changed" | "unchanged";
  reason: string;
  line: string | null;
}

export interface ReviewState {
  v: number;
  pr: number;
  head: string | null;
  base: string | null;
  status: ReviewStatus;
  verdict: Verdict | null;
  reason?: string;
  findings: Finding[];
  possible: Possible[];
  rounds: Round[];
  next: number;
  /** Set by a round whose general pass finished; absent on an older state or a skipped review. */
  roadmap?: RoadmapJudgement;
}

// ---- Model passes -------------------------------------------------------------------

export interface FindingsOutput {
  findings: RawFinding[];
  not_reviewed: string[];
}

/** The general pass's answer: findings plus the roadmap judgement (GENERAL_SCHEMA). */
export interface GeneralOutput extends FindingsOutput {
  roadmap?: { reasoning?: unknown; behaviour?: unknown; line?: unknown };
}

export interface VerifyVerdict {
  id: string;
  reason: string;
  verdict: "confirmed" | "rejected" | "uncertain";
  severity: Severity;
  duplicate_of?: string;
}

export interface VerifyOutput {
  results: VerifyVerdict[];
}

export interface RecheckResult {
  id: string;
  reason: string;
  status: FindingStatus;
}

export interface RecheckOutput {
  results: RecheckResult[];
}

export interface PassResult<T = unknown> {
  name: string;
  ok: boolean;
  subtype: string;
  output: T | null;
  cost: number;
  ms: number;
  turns: number;
  tools: Record<string, number>;
  error: string;
}

export interface PassSpec {
  name: string;
  prompt: string;
  schema: Record<string, unknown>;
  effort: Effort;
  taskTokens?: number;
}

export type Effort = "low" | "medium" | "high" | "xhigh" | "max";

export type RunPass = <T>(
  spec: PassSpec & { model: string; budgetUsd: number; cwd: string; graph: boolean }
) => Promise<PassResult<T>>;

// ---- GitHub -------------------------------------------------------------------------

export interface GhUser {
  login: string;
  type?: string;
}

export interface IssueComment {
  id?: number;
  node_id?: string;
  html_url?: string;
  body: string | null;
  created_at?: string;
  user: GhUser | null;
}

export interface ReviewComment {
  id: number;
  in_reply_to_id?: number | null;
  pull_request_review_id?: number;
  body: string | null;
  user: GhUser | null;
  reactions?: { "+1"?: number; "-1"?: number };
}

export interface PrReview {
  state: string;
  user: GhUser | null;
}

export interface PrMeta {
  number: number;
  title: string;
  body: string | null;
  draft: boolean;
  user: GhUser;
  labels: { name: string }[];
  head: { sha: string; ref?: string };
  base: { sha?: string; ref: string; repo?: { default_branch?: string } };
  merge_commit_sha?: string | null;
  merged_at?: string | null;
  additions?: number;
  deletions?: number;
}

export interface CommitStatus {
  state: "pending" | "success" | "failure";
  description: string;
}

export interface Thread {
  id: string;
  resolved: boolean;
  firstComment: number | undefined;
}

/** The GitHub calls the publishing code needs; github.ts implements them, tests fake them. */
export interface GitHub {
  createReview(repo: string, n: number, review: Record<string, unknown>): Promise<{ id: number }>;
  listReviewComments(repo: string, n: number): Promise<ReviewComment[]>;
  listReviewThreads(repo: string, n: number): Promise<Thread[]>;
  resolveThread(threadId: string): Promise<unknown>;
  replyToReviewComment(repo: string, n: number, commentId: number, body: string): Promise<unknown>;
  createIssueComment(repo: string, n: number, body: string): Promise<unknown>;
  updateIssueComment(repo: string, id: number, body: string): Promise<unknown>;
  minimizeComment(nodeId: string): Promise<unknown>;
}
