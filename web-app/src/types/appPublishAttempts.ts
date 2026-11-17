/** One pipeline stage's wall time. `stage` is one of `admit`, `inflate`,
 *  `store`, `record`, `promote`, `point`, `after` — or a name a newer server
 *  added, which is shown as it comes. */
export interface PublishStageTiming {
  stage: string;
  ms: number;
}

/**
 * One publish attempt, landed or refused — `GET /customer-apps/{id}/publishes`
 * (`admin::apps::publishes::PublishAttemptSummary`).
 */
export interface PublishAttempt {
  id: string;
  /** The build row a landed attempt recorded (`AppBuild.id`). Null for a failure,
   *  and once retention deleted the build. */
  app_build_id: string | null;
  build_id: string;
  /** `staging`, `production` or a sandbox `dev-<handle>`. */
  environment: string;
  outcome: "success" | "failed" | string;
  /** The stage a failed attempt failed at. */
  failed_stage: string | null;
  /** The refusal a failed attempt answered. */
  error: string | null;
  started_at: string;
  finished_at: string;
  duration_ms: number;
  /** Every stage that ran, in pipeline order. */
  stages: PublishStageTiming[];
  published_by_email: string | null;
  published_via: string | null;
  published_token_id: string | null;
}

export interface PublishAttemptList {
  attempts: PublishAttempt[];
}
