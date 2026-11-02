/**
 * The release timeline (`GET /customer-apps/{id}/releases`,
 * `crates/api-custom-apps/src/admin/apps/releases/dto.rs`): production cut into
 * releases, one per build it served, each beside the release before it, and the
 * three environments' pointer moves.
 */

export type ReleaseLaneName = "production" | "staging" | "sandboxes";
type PartState = "ok" | "unavailable";

interface ReleaseBuildRef {
  /** `app_builds.id`. */
  id: string;
  /** The publish's build id. */
  build_id: string;
  commit_sha: string | null;
  source_branch: string | null;
  source_repo: string | null;
  published_via: string | null;
}

export interface ReleaseLaneEvent {
  id: string;
  environment: string;
  action: string;
  at: string;
  build: ReleaseBuildRef | null;
  /** Its build has since been deleted. */
  pruned: boolean;
  actor: string | null;
  agent_built: boolean;
}

interface ReleaseLane {
  lane: ReleaseLaneName;
  events: ReleaseLaneEvent[];
}

interface ReleaseMetrics {
  calls: number;
  failed: number;
  shed: number;
  /** Basis points: 100 = 1 %. Null with no calls. */
  failure_rate_bp: number | null;
  route_calls: number;
  p50_ms: number | null;
  p95_ms: number | null;
}

export interface ReleaseScreenVitals {
  path: string;
  samples: number;
  lcp_p75: number | null;
  /** The worst interaction in a page's first 10 s, p75 across page loads. */
  inp_p75: number | null;
  /** CLS × 1000. */
  cls_p75: number | null;
}

interface ReleaseMigration {
  store: string;
  filename: string;
  applied_at: string;
  build_id: string | null;
}

interface ReleaseNewIssue {
  function_name: string;
  fingerprint: string;
  first_seen: string;
  occurrences: number;
}

export type VerdictKind = "worse" | "slower" | "not_worse" | "not_comparable";

export interface VerdictReason {
  metric: "failure_rate" | "p95" | "lcp" | "cls";
  screen: string | null;
  before: number;
  after: number;
  unit: "bp" | "ms" | "cls_milli";
  rule: string;
}

interface ReleaseVerdict {
  kind: VerdictKind;
  reasons: VerdictReason[];
  /** Set for `not_comparable`: why it could not be judged. */
  why: string | null;
}

interface ReleaseRollback {
  offered: boolean;
  /** The build a rollback goes back to: the release this one was compared with. */
  to: ReleaseBuildRef | null;
  why_not: "not_current" | "no_previous" | null;
}

export type ReleaseStartedBy =
  | "promote"
  | "publish"
  | "rollback"
  | "made_live"
  | "history_start"
  | "unpublish"
  | "other";

export interface Release {
  index: number;
  build: ReleaseBuildRef | null;
  pruned: boolean;
  unpublished: boolean;
  current: boolean;
  clipped: boolean;
  started_at: string;
  counted_from: string;
  ended_at: string | null;
  started_by: { action: ReleaseStartedBy; actor: string | null };
  metrics: ReleaseMetrics | null;
  vitals: ReleaseScreenVitals[];
  more_screens: boolean;
  migrations: ReleaseMigration[];
  new_issues: ReleaseNewIssue[];
  previous: number | null;
  verdict: ReleaseVerdict;
  /** Present on a red release only. */
  rollback: ReleaseRollback | null;
}

export interface ReleaseTimeline {
  window: { days: number; since: string; until: string };
  non_production: "included" | "not_permitted";
  lanes: ReleaseLane[];
  events_truncated: boolean;
  parts: { metrics: PartState; vitals: PartState; migrations: PartState; issues: PartState };
  /** Newest first. */
  releases: Release[];
  unreleased_migrations: ReleaseMigration[];
  migrations_truncated: boolean;
}
