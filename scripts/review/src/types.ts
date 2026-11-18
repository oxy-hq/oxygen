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

/** When a lens or a checklist item applies: `paths` match any changed file; `patch` is
 * tested against added lines, only in files matching `patchPaths` when given. */
export interface Trigger {
  paths?: readonly string[];
  patch?: RegExp;
  patchPaths?: readonly string[];
}

export interface Lens extends Trigger {
  id: string;
  prompt: string;
}

/**
 * One item of the incident checklist (checklist.ts): a question a past incident taught,
 * asked only of a diff whose shape calls for it, and the incident it came from.
 */
export interface ChecklistItem extends Trigger {
  id: string;
  /** What to check in this diff, in a sentence or two: the failure and how it shows. */
  check: string;
  /** The incident or PR behind it (`#3158`, or a doc section). An untraceable item is pruned. */
  source: string;
}

export type ChecklistPick = Pick<ChecklistItem, "id" | "check" | "source"> & { reason: string };

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
  /** Checklist items the diff triggers, for the general pass, capped at MAX_CHECKLIST. */
  checklist: ChecklistPick[];
  /** Ids of triggered items past the cap, named in the summary's Coverage. */
  droppedChecklist: string[];
  guidance: string[];
  /** Files under RULE_FILES this PR edits: every pass judges code by their merge-base copy. */
  editedRules: string[];
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
  suggestion?: unknown;
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
  /**
   * The exact new text of head lines `line`..`end_line`, posted as a GitHub suggestion
   * the author applies in one click. Kept only when the verifier vouched for it.
   */
  suggestion?: string;
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
  /** Only for the round that posts it inline; never stored (compactForStorage drops it). */
  suggestion?: string;
  /** Its inline comment offered a one-click suggestion: the metrics compare what that led to. */
  suggested?: boolean;
}

export interface Possible {
  title: string;
  file: string;
  line: number;
  severity: Severity;
  category: Category;
  why: string;
}

/** Tokens one pass used, summed over its turns (the SDK's per-model totals). */
export interface TokenUsage {
  /** Input read at full price: neither written to nor read from the prompt cache. */
  input: number;
  /** Written to the prompt cache (1.25x input price). */
  cacheWrite: number;
  /** Read from the prompt cache (a small fraction of input price). */
  cacheRead: number;
  /** Output, thinking included. */
  output: number;
}

/** The input side of a pass's first model call: whether it found the shared prefix cached. */
export type FirstCallUsage = Omit<TokenUsage, "output">;

export interface PassSummary {
  ok: boolean;
  subtype: string;
  cost: number;
  ms: number;
  turns: number;
  tools: Record<string, number>;
  /** Absent on rounds recorded before usage was kept. */
  usage?: TokenUsage;
  first?: FirstCallUsage;
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
  /** For a candidate with a suggestion: true only when applying it fixes exactly this. */
  suggestion_ok?: boolean;
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
  usage?: TokenUsage;
  first?: FirstCallUsage;
}

export interface PassSpec {
  name: string;
  prompt: string;
  schema: Record<string, unknown>;
  effort: Effort;
  taskTokens?: number;
  /**
   * Called once, as soon as the pass's first model response starts arriving: from then on
   * its prompt prefix is in the cache, so passes started after it read that prefix
   * instead of each writing it again (round.ts starts the others on this).
   */
  onFirstResponse?: () => void;
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

/** One entry of the API's PR file listing; `patch` is absent for a large or binary file. */
export interface PrFileListing {
  filename: string;
  status: string;
  previous_filename?: string;
  patch?: string;
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
