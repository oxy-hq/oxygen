import { TOKEN_KIND_LABELS } from "@/components/settings/SettingsDialog/sections/account/PersonalTokens/accessSummary";
import { tokenEnded } from "@/libs/tokenState";
import type { Token } from "@/types/apiToken";

export const KIND_LABELS = TOKEN_KIND_LABELS;

export const formatDay = (date: string | Date): string =>
  new Date(date).toLocaleDateString("en-US", { year: "numeric", month: "short", day: "numeric" });

const MINUTE = 60 * 1000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"}`;

/** "12 days", "3 hours", "5 minutes": the largest whole unit that fits. */
export function durationText(ms: number): string {
  if (ms >= DAY) return plural(Math.floor(ms / DAY), "day");
  if (ms >= HOUR) return plural(Math.floor(ms / HOUR), "hour");
  return plural(Math.max(1, Math.floor(ms / MINUTE)), "minute");
}

/** A token inside this many days of its expiry is worth a second look. */
const EXPIRING_SOON_DAYS = 7;

export type TokenTone = "active" | "soon" | "expired" | "revoked" | "removed";

export interface TokenLifecycle {
  tone: TokenTone;
  /** The badge: Active, Expired, Revoked or Removed. */
  label: string;
  /** The line beside it: when it ends, or ended. */
  detail: string;
}

type LifecycleFields = Pick<Token, "status" | "expires_at" | "revoked_at">;

/**
 * Where a token is in its life. The server's `status` is the authority;
 * `expires_at` is also checked against the clock so a row that lapsed while
 * the page was open doesn't keep reading as active.
 */
export function tokenLifecycle(token: LifecycleFields, now = new Date()): TokenLifecycle {
  // A legacy API key, which the format's removal ended on 2026-11-16 whatever
  // its row says. This is checked first and ignores `revoked_at` and
  // `expires_at` on purpose: the migration leaves the keys that were still in
  // use unrevoked, so that reverting the image can restore them, and reading
  // those rows as "Active, No expiry" would say the opposite of what the
  // server does.
  if (token.status === "removed") {
    return {
      tone: "removed",
      label: "Removed",
      detail: "Legacy API keys no longer authenticate"
    };
  }
  if (token.status === "revoked" || token.revoked_at) {
    return {
      tone: "revoked",
      label: "Revoked",
      detail: token.revoked_at ? `Revoked ${formatDay(token.revoked_at)}` : "No longer works"
    };
  }

  const expiresAt = token.expires_at ? new Date(token.expires_at) : null;
  const lapsed = expiresAt !== null && expiresAt.getTime() <= now.getTime();
  if (token.status === "expired" || lapsed) {
    return {
      tone: "expired",
      label: "Expired",
      detail: expiresAt ? `Expired ${formatDay(expiresAt)}` : "No longer works"
    };
  }

  if (!expiresAt) return { tone: "active", label: "Active", detail: "No expiry" };

  const remaining = expiresAt.getTime() - now.getTime();
  return {
    tone: remaining <= EXPIRING_SOON_DAYS * DAY ? "soon" : "active",
    label: "Active",
    detail: `Expires in ${durationText(remaining)}`
  };
}

type ActionFields = Pick<Token, "kind" | "status" | "expires_at">;

/**
 * Extend is offered on a live or lapsed token that has an expiry. Not on a
 * revoked one, not on one that never expires (`{days}` would *give* it an
 * expiry, which shortens it), and not on a trusted-access token, which lives
 * for fifteen minutes by design.
 */
export const isExtendable = (token: ActionFields): boolean =>
  token.kind !== "ci" && !tokenEnded(token) && token.expires_at !== null;

/** Regenerate swaps the secret and keeps the id and grants. Service-account tokens only here. */
export const isRegenerable = (token: ActionFields): boolean =>
  token.kind === "service_account" && !tokenEnded(token);

export const isRevocable = (token: ActionFields): boolean => !tokenEnded(token);

/** "Never", "Today", "Yesterday", "5 days ago", then the date. */
export function lastUsedText(lastUsedAt: string | null, now = new Date()): string {
  if (!lastUsedAt) return "Never";
  const days = Math.floor((now.getTime() - new Date(lastUsedAt).getTime()) / DAY);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  return formatDay(lastUsedAt);
}
