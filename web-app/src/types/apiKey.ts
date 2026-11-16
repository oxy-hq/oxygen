import type { AuditEvent } from "./audit";

/**
 * The row shape the shared credential pieces take: the status badge, the Extend
 * popover, the Activity drawer. It was a legacy API key as the legacy
 * `/{projectId}/api-keys` routes listed one; those went with the keys, and a
 * `Token` reaches this shape through `toTokenSummary`, so what is left is the
 * shape itself and the tests that build one.
 */
export interface ApiKey {
  id: string;
  name: string;
  expires_at?: string;
  last_used_at?: string;
  created_at: string;
  is_active: boolean;
  masked_key?: string;
}

/**
 * Body of `POST /user/tokens/{id}/extend` and the service-account equivalent.
 * Exactly one shape:
 * `days` (1–3650) counts from the later of now and the current expiry;
 * `expires_at` is an RFC 3339 instant in the future, or `null` for no expiry.
 */
export type ExtendApiKeyRequest = { days: number } | { expires_at: string | null };

/** One day of `api_token_usage_daily`. Days with no traffic may be absent. */
export interface ApiKeyUsageDay {
  /** `YYYY-MM-DD` (UTC). */
  day: string;
  requests: number;
  errors_4xx: number;
  errors_5xx: number;
}

/** The latest request made with a key. `route` is the matched template, never the raw path. */
export interface ApiKeyLastUsed {
  at: string;
  ip?: string | null;
  user_agent?: string | null;
  route?: string | null;
}

/**
 * The `/admin/audit` row shape. `metadata` is not part of that shape today; it is read
 * only to show a `token.extended` event's old → new expiry when the server sends it.
 */
export interface ApiKeyActivityEvent extends AuditEvent {
  metadata?: Record<string, unknown> | null;
}

/** `GET /{projectId}/api-keys/{id}/activity`. */
export interface ApiKeyActivityResponse {
  /** Newest first: lifecycle events on the key, and actions performed with it. */
  events: ApiKeyActivityEvent[];
  /** The last 30 days, oldest first. */
  usage: ApiKeyUsageDay[];
  /** `null` when the key has never been used. */
  last_used: ApiKeyLastUsed | null;
}
