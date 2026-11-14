/**
 * Workspace secrets shared with custom apps — `GET /{workspace_id}/shared-secrets`.
 *
 * A workspace secret is stored once; a workspace admin shares it with any of the
 * workspace's apps, and each reads it as `ctx.env.<NAME>`. Rotating it here reaches
 * every app it is shared with. An app's own value of the same key wins over a
 * shared one.
 */
export interface SharedSecret {
  secret_id: string;
  app_ids: string[];
}

/** A key an app's production build declares that nothing provides yet. */
export interface MissingAppKey {
  key: string;
  required: boolean;
  description?: string;
}

/** One app of the workspace, with what it still lacks. */
export interface AppSecretNeeds {
  app_id: string;
  app_name: string;
  app_slug: string;
  missing: MissingAppKey[];
}

export interface SharedSecretsResponse {
  /** Every workspace secret shared with at least one app. */
  shared: SharedSecret[];
  /** Every app of the workspace — drafts included — so names always resolve. */
  apps: AppSecretNeeds[];
}
