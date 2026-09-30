// Shared shapes for the release showcase. Design: internal-docs/release-showcase.md.

/**
 * The one identity every capture signs in as — the highest role that can still
 * enter the product, so a picture shows the whole feature and never a
 * permission error. The workflows put this address in OXY_GLOBAL_ADMINS (Global
 * Admin: every admin page but the owner-only two; `oxy seed` also makes it
 * owner of the Local org and its Demo workspace, so no assume-role session is
 * needed) and alone in OXY_DEV_LOGIN_EMAILS, which a release build needs.
 *
 * NOT OXY_OWNER: `OwnerRedirect` sends a Global Owner from every product route
 * — `/`, every `/<org>/…` workspace page — to the admin billing queue, so every
 * capture outside /admin would film that queue.
 */
export const SHOWCASE_USER = "showcase-staff@oxy.test";

/**
 * The core flow a showcase shows: the seeded Local org — its Demo workspace
 * built from `examples/` (agents, automations, data apps, the semantic model,
 * seeded threads) and its example custom apps — plus the admin console. The
 * seed's partner tenants exist on the instance but are not offered to the plan.
 */
export const SHOWCASE_ORG = "local";

export interface ShowcasePlan {
  /** `show`: capture it. `not_visual`: nothing on screen changed. `needs_seed`: the screen needs data the seed lacks. */
  verdict: "show" | "not_visual" | "needs_seed";
  /** One sentence: why this verdict. */
  reason: string;
  /** One plain sentence a non-engineer reads above the picture. */
  headline: string;
  /** Absolute path; workspace ids as `{ws:<org_slug>/<workspace name>}`. */
  start_path: string;
  /** Natural-language actions, clicking only — no URLs after the start page. */
  steps: string[];
  /** What the final screen must show; the judge's claim. */
  expect: string;
  media: "screenshot" | "video";
}

export type Outcome =
  | "captured"
  | "not_candidate"
  | "not_visual"
  | "needs_seed"
  | "rejected"
  | "failed"
  /** The spend limit stopped it; retrying the same thing would spend the same. */
  | "over_budget";

export interface ShowcaseRecord {
  version: 1;
  pr: number;
  title: string;
  head_sha: string;
  outcome: Outcome;
  /** Why this outcome, in a sentence. */
  reason: string;
  plan?: ShowcasePlan;
  /** File names inside the record directory. */
  actions?: string;
  screenshot?: string;
  video?: string;
  /** Everything this run spent, as the budget meter counted it. */
  cost_usd: number;
  captured_at: string;
  /** `uiHash` of what the run looked at; an unchanged hash reuses this record. */
  ui_hash?: string;
  /** A fault outside the plan (an API error, a broken boot): the next push tries again. */
  retryable?: boolean;
}

/** Pointer the PR comment carries so a release can find the record later. */
export interface RecordPointer {
  run_id: string;
  artifact: string;
  head_sha: string;
  outcome: Outcome;
  /** Slack thread timestamps this record was already posted into. */
  posted_in?: string[];
  ui_hash?: string;
  spent_usd?: number;
  retryable?: boolean;
}

export const RECORD_FILE = "record.json";
export const ACTIONS_FILE = "actions.json";
