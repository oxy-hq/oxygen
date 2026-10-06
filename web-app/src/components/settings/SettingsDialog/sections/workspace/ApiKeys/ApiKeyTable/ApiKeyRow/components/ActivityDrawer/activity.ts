import type { ApiKeyActivityEvent, ApiKeyUsageDay } from "@/types/apiKey";

/** `token.created|extended|revoked|…`: something done *to* the key, not *with* it. */
export const isLifecycleEvent = (e: Pick<ApiKeyActivityEvent, "action">): boolean =>
  e.action.startsWith("token.");

/** Split the newest-first stream into the key's history and the actions taken with it. */
export const splitEvents = (events: ApiKeyActivityEvent[]) => ({
  lifecycle: events.filter(isLifecycleEvent),
  actions: events.filter((e) => !isLifecycleEvent(e))
});

const LIFECYCLE_LABELS: Record<string, string> = {
  "token.created": "Created",
  "token.extended": "Extended",
  "token.regenerated": "Regenerated",
  "token.revoked": "Revoked"
};

/** "Extended"; an action this build doesn't know yet reads as its own name, de-dotted. */
export const lifecycleLabel = (action: string): string => {
  const known = LIFECYCLE_LABELS[action];
  if (known) return known;
  const rest = action.replace(/^token\./, "").replace(/[._]/g, " ");
  return rest.charAt(0).toUpperCase() + rest.slice(1);
};

/** An expiry change; `null` on either side means "no expiry". */
export interface ExpiryChange {
  from: string | null;
  to: string | null;
}

/**
 * Key pairs a `token.extended` event may carry its old and new expiry under. The `/admin/audit`
 * row has no `metadata`, so this is read defensively: an event without either key shows no change.
 */
const EXPIRY_KEY_PAIRS: [string, string][] = [
  ["old_expires_at", "new_expires_at"],
  ["previous_expires_at", "new_expires_at"],
  ["from_expires_at", "to_expires_at"],
  ["from", "to"]
];

const asExpiry = (v: unknown): string | null | undefined => {
  if (v === null) return null;
  return typeof v === "string" && v ? v : undefined;
};

export const expiryChange = (e: ApiKeyActivityEvent): ExpiryChange | null => {
  const meta = e.metadata;
  if (!meta || typeof meta !== "object") return null;
  for (const [fromKey, toKey] of EXPIRY_KEY_PAIRS) {
    if (!(fromKey in meta) || !(toKey in meta)) continue;
    const from = asExpiry(meta[fromKey]);
    const to = asExpiry(meta[toKey]);
    if (from !== undefined && to !== undefined) return { from, to };
  }
  return null;
};

const DAY_MS = 24 * 60 * 60 * 1000;
const isoDay = (ms: number) => new Date(ms).toISOString().slice(0, 10);

/**
 * Exactly `days` consecutive UTC days ending today (or at the newest day the server sent, if
 * its clock is ahead), zero-filling the days it omitted for having no traffic.
 */
export const fillUsageDays = (
  usage: ApiKeyUsageDay[],
  now = new Date(),
  days = 30
): ApiKeyUsageDay[] => {
  const byDay = new Map(usage.map((u) => [u.day, u]));
  const today = isoDay(now.getTime());
  const newest = usage.reduce((max, u) => (u.day > max ? u.day : max), today);
  const end = Date.parse(`${newest}T00:00:00Z`);
  return Array.from({ length: days }, (_, i) => {
    const day = isoDay(end - (days - 1 - i) * DAY_MS);
    return byDay.get(day) ?? { day, requests: 0, errors_4xx: 0, errors_5xx: 0 };
  });
};

export interface UsageTotals {
  requests: number;
  ok: number;
  errors4xx: number;
  errors5xx: number;
}

/** Requests that were neither 4xx nor 5xx. Never negative, whatever the server sums to. */
export const okCount = (d: ApiKeyUsageDay): number =>
  Math.max(0, d.requests - d.errors_4xx - d.errors_5xx);

export const usageTotals = (days: ApiKeyUsageDay[]): UsageTotals =>
  days.reduce<UsageTotals>(
    (t, d) => ({
      requests: t.requests + d.requests,
      ok: t.ok + okCount(d),
      errors4xx: t.errors4xx + d.errors_4xx,
      errors5xx: t.errors5xx + d.errors_5xx
    }),
    { requests: 0, ok: 0, errors4xx: 0, errors5xx: 0 }
  );
