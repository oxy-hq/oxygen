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

/** The table's filter: every secret, or one kind of who-can-read-it. */
export type SecretsFilter = "all" | "shared" | "workspace" | "app";

export const DOTS = "••••••••••••••";
