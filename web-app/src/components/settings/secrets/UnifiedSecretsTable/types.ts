import type { Secret } from "@/types/secret";

/** A single row in the secrets table: a secret stored in the database. */
export interface UnifiedRow {
  key: string;
  /** What the row is called on screen. For an app secret this is the bare key
   *  (`STRIPE_API_KEY`), not the `apps/<uuid>/` name it is stored under. */
  name: string;
  /** Set when the row is an app-scoped secret — the app it belongs to. */
  app?: { id: string; name: string };
  source: "secret";
  secretInfo: Secret;
}

export const SOURCE_CONFIG = {
  secret: { label: "Secret", className: "border-info/30 bg-info/10 text-info" }
} as const;

export const DOTS = "••••••••••••••";
