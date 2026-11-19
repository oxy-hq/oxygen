/**
 * What an organization's custom apps may do in Oxygen itself: the platform
 * actions they ask for (`"oxygen": { … }` in `oxy-app.json`, called as
 * `ctx.oxygen.*`) and an org admin's approval of them.
 * `GET /api/orgs/{org_id}/oxygen-access` and its approve and revoke routes.
 *
 * Not the other app-access feature (`./appAccess.ts`), which is about which
 * teams and people may open an app.
 */

/** `pending`: asked for, not approved. `approved_unused`: approved, no longer asked for. */
export type OxygenActionStatus = "pending" | "approved" | "approved_unused";

/** One pipeline an upload names, with its own status. */
export interface OxygenTarget {
  name: string;
  status: OxygenActionStatus;
  approved_at: string | null;
  approved_by: string | null;
}

export interface OxygenAction {
  /** The action's stable id, e.g. `frontline_device_manage`. */
  action: string;
  /** What it lets the app do, in the platform's words. */
  description: string;
  /** The live build's functions that ask for it. */
  functions: string[];
  /** Pending while anything it lists (an upload's pipelines) is not approved. */
  status: OxygenActionStatus;
  approved_at: string | null;
  approved_by: string | null;
  /** What it is approved on: an upload's pipelines, by name. Empty for the whole organization. */
  targets: OxygenTarget[];
}

/** Who put the live build there: who the approver is trusting. */
export interface OxygenLiveBuild {
  build_id: string;
  published_at: string;
  published_by: string | null;
  published_via: string | null;
}

export interface OxygenAccess {
  app_id: string;
  app_slug: string;
  app_name: string;
  live_build: OxygenLiveBuild | null;
  actions: OxygenAction[];
  /** The app's row in the token inventory while it holds a live approval. */
  token_id: string | null;
  /** The service account the app acts as, once an approval made one. */
  service_account: string | null;
  last_used_at: string | null;
}
