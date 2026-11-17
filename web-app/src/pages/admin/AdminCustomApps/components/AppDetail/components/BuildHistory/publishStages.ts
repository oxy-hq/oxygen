import type { PublishAttempt, PublishStageTiming } from "@/types/appPublishAttempts";

/** A duration as the stage bar labels it: `12ms` under a second, `1.2s` under ten,
 *  whole seconds past that. */
export function formatMs(ms: number): string {
  const n = Number.isFinite(ms) ? Math.max(0, ms) : 0;
  if (n < 1000) return `${Math.round(n)}ms`;
  const s = n / 1000;
  return s < 10 ? `${s.toFixed(1)}s` : `${Math.round(s)}s`;
}

/** The sum of every stage's time. */
export function totalMs(stages: PublishStageTiming[]): number {
  return stages.reduce((sum, s) => sum + Math.max(0, s.ms), 0);
}

export interface StageSegment {
  stage: string;
  ms: number;
  /** The stage's fraction of the whole publish, `0..1`; the shares sum to 1. */
  share: number;
  /** The stage the publish was refused at. */
  failed: boolean;
}

/**
 * The stage bar's segments, in the order the stages ran, each sized by its share
 * of the total. A stage that took no measurable time has no share and draws no
 * segment, but keeps its label. A publish whose every stage was under a
 * millisecond splits the bar evenly rather than drawing nothing.
 */
export function stageSegments(
  stages: PublishStageTiming[],
  failedStage?: string | null
): StageSegment[] {
  const total = totalMs(stages);
  return stages.map((s) => ({
    stage: s.stage,
    ms: s.ms,
    share: total > 0 ? Math.max(0, s.ms) / total : 1 / stages.length,
    failed: !!failedStage && s.stage === failedStage
  }));
}

/** The attempts that failed, in the order served (newest first). */
export function failedAttempts(attempts: PublishAttempt[] | undefined): PublishAttempt[] {
  return (attempts ?? []).filter((a) => a.outcome === "failed");
}

/**
 * The attempt that landed each build, by its build row id (`AppBuild.id`), for the
 * ledger's per-build bar. The attempts are newest first, so if two name one build
 * the newer wins; a failed attempt names no build and is never here.
 */
export function landedByBuild(attempts: PublishAttempt[] | undefined): Map<string, PublishAttempt> {
  const byBuild = new Map<string, PublishAttempt>();
  for (const attempt of attempts ?? []) {
    if (attempt.outcome !== "success" || !attempt.app_build_id) continue;
    if (!byBuild.has(attempt.app_build_id)) byBuild.set(attempt.app_build_id, attempt);
  }
  return byBuild;
}

/** Who made an attempt, as the ledger names a publisher: the person, else the
 *  verified workflow of a trusted (OIDC) publish, else the sandbox agent token. */
export function publisherLabel(attempt: PublishAttempt): string | null {
  if (attempt.published_by_email) return attempt.published_by_email;
  if (attempt.published_via) return attempt.published_via;
  if (attempt.published_token_id) return "a sandbox agent token";
  return null;
}
