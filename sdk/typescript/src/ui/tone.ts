// Status tones for `@oxy-hq/sdk/ui`: the one set of words a desk piece colours by, the classes that
// paint each, and the rule for which tone a group wears.
//
// Started from oxy-hq/reference-app@3caad1f `src/components/ui.ts:458-545` (`ChipTone`,
// `chipTone`, `StatusTone`, `calloutTone`) and `DefaultTabs.tsx:123` (`strongestTone`), checked
// against Store Ops' recipe module (pokehouse-oxy `apps/store-ops/src/chrome/tokens.ts:127-157`),
// which added `toneText` and `toneFill`.
//
// Shared with the phone kit, not forked from it: the phone kit names the same statuses
// good / warn / bad / neutral (`src/shell/phone/tone.ts`), and the two maps below are the only
// place one vocabulary becomes the other. `present` is the kit's own.

import type { PhoneTone } from "../shell/phone/tone";

export { present } from "../shell/phone/tone";

/**
 * What a mark says. `neutral` reports a state rather than a verdict ("everything posted"); it is
 * not a status hue, so it draws in the dim ink on the neutral tint. Colour is never the only
 * channel: every toned piece also prints its status as a word.
 */
export type StatusTone = "success" | "warning" | "error" | "info" | "neutral";

/** The reference's name for the same set, kept so a move is an import swap. */
export type ChipTone = StatusTone;

/** Every tone, in the order the gallery shows them. */
export const STATUS_TONES: readonly StatusTone[] = [
  "success",
  "warning",
  "error",
  "info",
  "neutral"
];

/** A tag's or a count's tone: the status tint behind the status ink (base.css `.oxy-ui-tone-*`). */
export const chipTone: Record<StatusTone, string> = {
  success: "oxy-ui-tone-success",
  warning: "oxy-ui-tone-warning",
  error: "oxy-ui-tone-error",
  info: "oxy-ui-tone-info",
  neutral: "oxy-ui-tone-neutral"
};

/** A callout's tone: the tint, the same hue's line at low alpha, the status ink. Pair with
 *  `ui.callout`. */
export const calloutTone: Record<StatusTone, string> = {
  success: "oxy-ui-callout--success",
  warning: "oxy-ui-callout--warning",
  error: "oxy-ui-callout--error",
  info: "oxy-ui-callout--info",
  neutral: "oxy-ui-callout--neutral"
};

/** The ink alone, for a status word in running text. */
export const toneText: Record<StatusTone, string> = {
  success: "oxy-ui-ink-success",
  warning: "oxy-ui-ink-warning",
  error: "oxy-ui-ink-error",
  info: "oxy-ui-ink-info",
  neutral: "oxy-ui-ink-neutral"
};

/** A solid status mark (a dot, a disc, a bar): the chart cut of each hue. */
export const toneFill: Record<StatusTone, string> = {
  success: "oxy-ui-fill-success",
  warning: "oxy-ui-fill-warning",
  error: "oxy-ui-fill-error",
  info: "oxy-ui-fill-info",
  neutral: "oxy-ui-fill-neutral"
};

/**
 * The tone a group's count wears: the most severe status among what it holds. Error over warning
 * over everything else, which reads as neutral (success and info included: "nothing to do here" is
 * not news a count needs to shout). Returns `undefined` for neutral, so the caller's neutral count
 * takes over.
 */
export function strongestTone(
  tones: Iterable<StatusTone | undefined | null>
): StatusTone | undefined {
  let worst: StatusTone | undefined;
  for (const t of tones) {
    if (t === "error") return "error";
    if (t === "warning") worst = "warning";
  }
  return worst;
}

/** The phone kit's word for a desk tone. The kit has no info hue: info is neutral there. */
export const phoneToneOf: Record<StatusTone, PhoneTone> = {
  success: "good",
  warning: "warn",
  error: "bad",
  info: "neutral",
  neutral: "neutral"
};

/** The desk tone for a phone kit tone. */
export const statusToneOf: Record<PhoneTone, StatusTone> = {
  good: "success",
  warn: "warning",
  bad: "error",
  neutral: "neutral"
};
