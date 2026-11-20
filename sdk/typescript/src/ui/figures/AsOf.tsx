// New: BEYOND-GALLERY row 4 (a freshness stamp in 13 files across 9 repos, worded and formatted
// thirteen ways, 0 tests). The words come from `@oxy-hq/sdk/format` (`formatAsOf`,
// `formatRelative`, `formatDataThrough`), so a stamp reads the same in a page header, a card and
// a Function's email. The look is the `note` recipe, as Supply Chain's and Marketing's page
// headers draw theirs (`apps/supply-chain/src/components/PageHeader.tsx:26`). Command Center's
// `AsOfChip` (`apps/command-center/src/components/exec/AsOfChip.tsx`) is the one with a second
// state, the feed behind: here `behind`, in the warning ink and with the word, never colour alone.
//
// How old the figures are, in one of three wordings:
//   asOf          "As of 3:03 PM" (today), "As of Nov 18, 3:03 PM", "As of Nov 18, 2025, 3:03 PM"
//   updated       "Updated just now", "Updated 3m ago", "Updated yesterday", "Updated Nov 2"
//   dataThrough   "Data through Nov 18": the newest day the figures cover
//
// **Where it sits** is the caller's: decision 48 puts a page action and its freshness
// ("Refresh from QuickBooks · As of 3:03 PM") outside the tab strip. Tabs hold only tabs.
//
// **The clock.** Given `now`, the stamp is fixed (a test, a server render, a screenshot). Without
// it, the stamp reads the time itself and re-renders on a timer, but only while it can be seen:
// the page is visible and the stamp is on screen (an IntersectionObserver, where there is one).
// A hidden tab or a stamp scrolled away costs nothing, and catches up the moment it shows.

import { type ComponentPropsWithRef, useEffect, useRef, useState } from "react";
import {
  type DateInput,
  formatAsOf,
  formatDataThrough,
  formatDateTime,
  formatRelative,
  MISSING
} from "../../format";
import { cx } from "../cx";
import { ui } from "../recipes";

export type AsOfKind = "asOf" | "updated" | "dataThrough";

export interface AsOfProps extends Omit<ComponentPropsWithRef<"time">, "children" | "dateTime"> {
  /** When the figures were read (an instant), or the newest day they cover (`"2026-11-18"`). */
  at: DateInput | null | undefined;
  /** The wording. Default `asOf`. */
  kind?: AsOfKind;
  /** The present. Fixes the stamp; leave it out to let the stamp keep its own time. */
  now?: DateInput;
  /** The zone the stamp is read in (`"America/Los_Angeles"`). Default UTC, as the format entry. */
  timeZone?: string;
  /** The feed is behind: the stamp says so in words and in the warning ink. */
  behind?: boolean;
  /** The word for `behind`. Default "Feed behind". */
  behindLabel?: string;
}

/** How often a live stamp re-reads the clock: the relative form counts minutes. */
const TICK_MS: Record<AsOfKind, number> = { updated: 30_000, asOf: 60_000, dataThrough: 60_000 };

/** The stamp's words for `at` at `now`, or null when `at` cannot be read. */
export function asOfText(
  at: DateInput | null | undefined,
  kind: AsOfKind,
  now: DateInput,
  timeZone?: string
): string | null {
  const zone = timeZone === undefined ? {} : { timeZone };
  if (kind === "updated") {
    const ago = formatRelative(at, now, zone);
    if (ago === MISSING) return null;
    return `Updated ${ago === "now" ? "just now" : ago}`;
  }
  const text =
    kind === "dataThrough"
      ? formatDataThrough(at, { ...zone, now })
      : formatAsOf(at, { ...zone, now });
  return text === MISSING ? null : text;
}

/** The time, re-read every `ms` while the element can be seen; fixed when `fixed` is given. */
function useLiveNow(fixed: DateInput | undefined, ms: number, target: { current: Element | null }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (fixed !== undefined) return;
    let onScreen = true;
    let timer: ReturnType<typeof setInterval> | undefined;
    const pageVisible = () => document.visibilityState !== "hidden";
    const sync = () => {
      const run = onScreen && pageVisible();
      if (run && timer === undefined) {
        setNow(Date.now());
        timer = setInterval(() => setNow(Date.now()), ms);
      } else if (!run && timer !== undefined) {
        clearInterval(timer);
        timer = undefined;
      }
    };
    const observer =
      typeof IntersectionObserver === "function" && target.current
        ? new IntersectionObserver((entries) => {
            onScreen = entries.some((e) => e.isIntersecting);
            sync();
          })
        : undefined;
    if (observer && target.current) observer.observe(target.current);
    document.addEventListener("visibilitychange", sync);
    sync();
    return () => {
      document.removeEventListener("visibilitychange", sync);
      observer?.disconnect();
      if (timer !== undefined) clearInterval(timer);
    };
  }, [fixed, ms, target]);
  return fixed ?? now;
}

export function AsOf({
  at,
  kind = "asOf",
  now,
  timeZone,
  behind = false,
  behindLabel = "Feed behind",
  className,
  ...rest
}: AsOfProps) {
  const ref = useRef<HTMLTimeElement | null>(null);
  const present = useLiveNow(now, TICK_MS[kind], ref);
  const text = asOfText(at, kind, present, timeZone);
  if (text === null || at === null || at === undefined) return null;
  const iso = typeof at === "string" ? at : new Date(at).toISOString();
  const full = formatDateTime(at, timeZone === undefined ? {} : { timeZone });
  const own = rest.ref;
  return (
    <time
      {...rest}
      ref={(el) => {
        ref.current = el;
        if (typeof own === "function") own(el);
        else if (own) own.current = el;
      }}
      dateTime={iso}
      title={rest.title ?? full}
      className={cx(ui.note, "oxy-ui-as-of", behind && ui.toneText.warning, className)}
      data-kind={kind}
      data-behind={behind ? "" : undefined}
    >
      {behind ? `${behindLabel} · ${text}` : text}
    </time>
  );
}
