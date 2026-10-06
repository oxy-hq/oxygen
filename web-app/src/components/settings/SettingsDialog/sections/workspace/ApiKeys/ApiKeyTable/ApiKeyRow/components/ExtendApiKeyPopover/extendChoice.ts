import { ApiKeyService } from "@/services/api/apiKey";
import type { ExtendApiKeyRequest } from "@/types/apiKey";

/** What the person picked in the Extend popover. */
export type ExtendChoice =
  | { kind: "days"; days: number }
  | { kind: "date"; date?: Date }
  | { kind: "never" };

/** The relative options, in the order shown. "1 year" is 365 days, as the server counts it. */
export const EXTEND_PRESETS = [
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
  { days: 365, label: "1 year" }
] as const;

export const DEFAULT_EXTEND_CHOICE: ExtendChoice = { kind: "days", days: 30 };

/** The end of the picked local day: a key "expiring Jan 31" still works on Jan 31. */
export const endOfLocalDay = (date: Date): Date => {
  const end = new Date(date);
  end.setHours(23, 59, 59, 999);
  return end;
};

/** The first day the date picker offers. The server refuses a past date; today is excluded too. */
export const firstPickableDay = (now = new Date()): Date => {
  const tomorrow = new Date(now);
  tomorrow.setHours(0, 0, 0, 0);
  tomorrow.setDate(tomorrow.getDate() + 1);
  return tomorrow;
};

/**
 * The expiry a choice would produce: a Date, `null` for no expiry, or `undefined` while the
 * date option has no date yet.
 */
export const previewExpiry = (
  token: { expires_at?: string | null },
  choice: ExtendChoice,
  now = new Date()
): Date | null | undefined => {
  switch (choice.kind) {
    case "days":
      return ApiKeyService.extendedExpiry(token.expires_at, choice.days, now);
    case "date":
      return choice.date ? endOfLocalDay(choice.date) : undefined;
    case "never":
      return null;
  }
};

/** The request body for a choice, or `undefined` when it isn't complete yet. */
export const buildExtendRequest = (choice: ExtendChoice): ExtendApiKeyRequest | undefined => {
  switch (choice.kind) {
    case "days":
      return { days: choice.days };
    case "date":
      return choice.date ? { expires_at: endOfLocalDay(choice.date).toISOString() } : undefined;
    case "never":
      return { expires_at: null };
  }
};

/** The submit button says exactly what will happen. */
export const submitLabel = (preview: Date | null | undefined): string => {
  if (preview === undefined) return "Pick a date";
  if (preview === null) return "Remove expiry";
  return `Extend to ${ApiKeyService.formatDay(preview)}`;
};
