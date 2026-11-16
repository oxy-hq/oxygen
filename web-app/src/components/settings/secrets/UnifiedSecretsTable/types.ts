import type { Secret } from "@/types/secret";

/** An app as a secrets row names it. */
export interface AppRef {
  id: string;
  name: string;
}

/** A single row in the secrets table: a secret stored in the database. */
export interface UnifiedRow {
  key: string;
  /** What the row is called on screen. For an app secret this is the bare key
   *  (`STRIPE_API_KEY`), not the `apps/<uuid>/` name it is stored under. */
  name: string;
  /** Set when the row is an app's own secret — the one app that reads it. */
  app?: AppRef;
  /** For a workspace secret: the apps it is shared with, each reading it as
   *  `ctx.env.<name>`. Always empty for an app's own secret. */
  sharedWith: AppRef[];
  secretInfo: Secret;
}

/**
 * Which tab a row belongs to. The two are different things, not two views of
 * one thing: a **workspace** secret is stored once and read by the workspace
 * and by whichever apps it is shared with, while an **app** secret is one
 * app's own `apps/<id>/<KEY>` and reaches nothing else.
 */
export type SecretsScope = "workspace" | "app";

/** Within the Workspace tab: every secret, or by whether any app reads it. */
export type SecretsFilter = "all" | "shared" | "unshared";

export const DOTS = "••••••••••••••";
