import {
  endOfLocalDay,
  firstPickableDay
} from "@/components/settings/SettingsDialog/sections/workspace/ApiKeys/ApiKeyTable/ApiKeyRow/components/ExtendApiKeyPopover/extendChoice";
import type { ExpiryInput } from "@/types/apiToken";
import type { LifetimeCap } from "./accessDraft";

export const DAY_MS = 24 * 60 * 60 * 1000;

/** What the person picked for a new token's lifetime. */
export type ExpiryChoice =
  | { kind: "days"; days: number }
  | { kind: "date"; date?: Date }
  | { kind: "never" };

/** The relative options, in the order shown. "1 year" is 365 days, as the server counts it. */
export const EXPIRY_PRESETS = [
  { days: 7, label: "7 days" },
  { days: 30, label: "30 days" },
  { days: 90, label: "90 days" },
  { days: 365, label: "1 year" }
] as const;

export interface ExpiryOption {
  days: number;
  label: string;
}

const daysLabel = (days: number): string => `${days} day${days === 1 ? "" : "s"}`;

/**
 * The relative lifetimes on offer: the presets, plus the cap itself when it falls between them.
 * Otherwise a 45-day cap would leave 30 days as the longest thing to pick.
 */
export const expiryOptions = (cap: LifetimeCap | null): readonly ExpiryOption[] => {
  const longest = EXPIRY_PRESETS[EXPIRY_PRESETS.length - 1].days;
  if (!cap || cap.days >= longest || EXPIRY_PRESETS.some((preset) => preset.days === cap.days)) {
    return EXPIRY_PRESETS;
  }
  return [...EXPIRY_PRESETS, { days: cap.days, label: daysLabel(cap.days) }].sort(
    (a, b) => a.days - b.days
  );
};

/** 90 days: what the server gives a token created with no expiry field at all. */
export const DEFAULT_EXPIRY_CHOICE: ExpiryChoice = { kind: "days", days: 90 };

/** The expiry half of the create body, or `undefined` while the date option has no date. */
export const expiryInput = (choice: ExpiryChoice): ExpiryInput | undefined => {
  switch (choice.kind) {
    case "days":
      return { expires_in_days: choice.days };
    case "date":
      return choice.date ? { expires_at: endOfLocalDay(choice.date).toISOString() } : undefined;
    case "never":
      return { expires_at: null };
  }
};

/** When the token would expire: a Date, `null` for never, `undefined` while no date is picked. */
export const expiryPreview = (choice: ExpiryChoice, now = new Date()): Date | null | undefined => {
  switch (choice.kind) {
    case "days":
      return new Date(now.getTime() + choice.days * DAY_MS);
    case "date":
      return choice.date ? endOfLocalDay(choice.date) : undefined;
    case "never":
      return null;
  }
};

/** Whether a preset fits under an org's max lifetime. No cap, everything fits. */
export const presetAllowed = (days: number, cap: LifetimeCap | null): boolean =>
  !cap || days <= cap.days;

/** The last calendar day whose end is still inside the cap: the date picker's upper bound. */
export const lastPickableDay = (cap: LifetimeCap, now = new Date()): Date => {
  const limit = now.getTime() + cap.days * DAY_MS;
  const day = new Date(limit);
  day.setHours(0, 0, 0, 0);
  if (endOfLocalDay(day).getTime() > limit) day.setDate(day.getDate() - 1);
  return day;
};

/** The days the date picker offers: from tomorrow, up to the cap if there is one. */
export const pickableRange = (
  cap: LifetimeCap | null,
  now = new Date()
): { before: Date; after?: Date } => ({
  before: firstPickableDay(now),
  ...(cap ? { after: lastPickableDay(cap, now) } : {})
});

const capSentence = (cap: LifetimeCap): string =>
  `${cap.orgName} limits tokens to ${daysLabel(cap.days)}`;

/**
 * Why the chosen expiry can't be used under the cap, or `null`. The date picker and the presets
 * already keep people inside it; this catches a choice made before the org was picked.
 */
export const expiryProblem = (
  choice: ExpiryChoice,
  cap: LifetimeCap | null,
  now = new Date()
): string | null => {
  if (!cap) return null;
  const expires = expiryPreview(choice, now);
  if (expires === undefined) return null;
  if (expires === null) return `${capSentence(cap)}, so this token needs an expiry.`;
  return expires.getTime() > now.getTime() + cap.days * DAY_MS
    ? `${capSentence(cap)}. Pick a shorter expiry.`
    : null;
};

/**
 * The choice to fall back to when a cap arrives and the current one no longer fits: the longest
 * option under the cap, which is the cap itself when no preset equals it.
 */
export const fitChoiceToCap = (
  choice: ExpiryChoice,
  cap: LifetimeCap | null,
  now = new Date()
): ExpiryChoice => {
  if (expiryProblem(choice, cap, now) === null) return choice;
  const fitting = expiryOptions(cap).filter((option) => presetAllowed(option.days, cap));
  const longest = fitting[fitting.length - 1];
  return longest ? { kind: "days", days: longest.days } : { kind: "date" };
};
