// Started from pokehouse-oxy:apps/store-ops/src/chrome/DefaultTabs.tsx + stripFit.ts (INVENTORY:
// the only copy with tests, and the only one that does what the reference's guidance says of a
// strip that outgrows its room: it becomes a select, decisions 22, 29). Prop docs, `legend` and
// `trailing` from oxy-hq/reference-app@3caad1f `src/components/DefaultTabs.tsx`. Kept: the
// WAI-ARIA tabs keyboard (one Tab stop, Left/Right wrap, Home/End), `activation="manual"`, the
// select swap, `overflow="scroll"` with the selected tab kept in view under the end fade (crew
// and the enrolled store tablet, round 16d), `size="touch"`, the inset focus ring (the strip
// clips one drawn outside), `TabPanel`. Changed: Store Ops' `KioskDeviceContext` is the app's,
// so a kiosk passes `overflow="scroll"` itself; a count is drawn whenever it is a number, a 0
// included, as the reference draws it (omit `count` for options that have no size).
//
// The app's DEFAULT tabs: which SLICE of this screen am I looking at. The pair to `ParentTabs`,
// and the two names carry the rule between them: a PARENT strip changes which screen you are
// on, a DEFAULT strip re-slices the screen you are already on, even when the choice is mirrored
// into the URL query. Not for a choice that goes to another route (a destination), not for many
// or long options or a form answer (`Segmented`, a select), and not for options that can be on
// together.
//
// The WAI-ARIA tabs pattern: the strip is one Tab stop (the selected tab), Left/Right move
// along it and wrap, Home/End jump to the ends. Selection follows focus, because a re-slice
// costs nothing, except where switching does real work (it can open a discard-changes dialog):
// pass `activation="manual"`, and the arrows move focus while Enter or Space selects.
//
// A count wears the most severe status among what its slice holds: pass
// `countTone={strongestTone(rows.map(toneOf))}`; neutral when undefined. It shouts only error and
// warning (the tone rule, decision 31): success and info are not news a count needs to shout, so
// they draw neutral, as `strongestTone` already reads them. Tabs hold only tabs
// (decision 48): a page action or its freshness stamp sits outside the strip.

import { type KeyboardEvent, type ReactNode, type RefObject, useLayoutEffect } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";
import { chipTone, type StatusTone } from "../tone";
import { optionText, useStripFits } from "./stripFit";

export interface TabOption<T extends string> {
  value: T;
  /** Words: the select that replaces a strip too wide for its room shows them. */
  label: string;
  /** The slot in front of the label. Decorative and `aria-hidden` unless it is the tab's state
   *  the label does not say; the call site decides. */
  icon?: ReactNode;
  /** How many the slice holds, drawn whenever it is a number. Omit for options that have no
   *  size (seven days, not seven counts). */
  count?: number | null;
  /** The most severe status among what the count holds (`strongestTone`). Error and warning
   *  tint the count; anything else, or none, draws it neutral. */
  countTone?: StatusTone;
  /** Anything richer than a count, in the count's place; wins over `count`. */
  trailing?: ReactNode;
  /** Hover text for the tab. */
  title?: string;
}

export interface DefaultTabsProps<T extends string> {
  /** Namespaces the tab ids; pair with `TabPanel` (or `panelId`). */
  group: string;
  /** The strip's own accessible name: "Which stores", "Day". */
  label: string;
  value: T;
  options: readonly TabOption<T>[];
  onChange: (value: T) => void;
  /** A key to what the strip's panel is about to show, on the strip's own row. */
  legend?: ReactNode;
  /** The strip shares a row with 32px controls, so it takes their height. */
  sharesRow?: boolean;
  /** The caller renders a `TabPanel` for this group, so each tab can point at it. */
  hasPanel?: boolean;
  /** `manual` where selecting does real work: the arrows only move focus. */
  activation?: "auto" | "manual";
  /** `touch` on a store tablet: the same strip at the 56px crew target and crew type, full
   *  width, each tab an equal share. */
  size?: "compact" | "touch";
  /**
   * What a strip wider than its room does. `select` (the desk): it gives way to a select.
   * `scroll` (crew mode, an enrolled store tablet, whose layout does not move, decision 1): it
   * stays a strip, scrolls sideways under an end fade, and keeps the selected tab in view.
   */
  overflow?: "select" | "scroll";
  className?: string;
}

/** A value as an id fragment: an id holds no whitespace, and `aria-labelledby` splits on it. */
const slug = (value: string) => value.trim().replace(/\s+/g, "-");

/** The id of one tab, for the panel's `aria-labelledby`. One function, so the two ends cannot
 *  drift: a panel pointing at an id that does not exist fails silently. */
export function tabId(group: string, value: string): string {
  return `${group}-tab-${slug(value)}`;
}

/** The id of a strip's one panel. */
export function panelId(group: string): string {
  return `${group}-panel`;
}

/**
 * Scroll a sideways-scrolling strip so its selected tab is wholly in view, and clear of the end
 * fade while the strip is not at its end. Only `scrollLeft` moves: the page never scrolls for it.
 */
function keepSelectedInView(el: HTMLElement, index: number) {
  if (el.scrollWidth <= el.clientWidth + 1) return;
  const tab = el.querySelectorAll<HTMLElement>('[role="tab"]')[index];
  if (!tab) return;
  const box = el.getBoundingClientRect();
  const left = box.left + el.clientLeft;
  const right = left + el.clientWidth;
  const fade = parseFloat(getComputedStyle(el, "::after").width) || 0;
  const t = tab.getBoundingClientRect();
  if (t.left < left) el.scrollLeft -= left - t.left;
  else if (t.right > right - fade) el.scrollLeft += t.right - (right - fade);
}

/** While `on`: the selected tab in view when it changes, and when the strip's room does. */
function useSelectedInView(strip: RefObject<HTMLElement | null>, index: number, on: boolean) {
  useLayoutEffect(() => {
    const el = strip.current;
    if (!on || !el) return;
    keepSelectedInView(el, index);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => keepSelectedInView(el, index));
    ro.observe(el);
    return () => ro.disconnect();
  }, [strip, index, on]);
}

const KEYS: Record<string, (index: number, last: number) => number> = {
  ArrowRight: (i, last) => (i === last ? 0 : i + 1),
  ArrowLeft: (i, last) => (i === 0 ? last : i - 1),
  Home: () => 0,
  End: (_i, last) => last
};

export function DefaultTabs<T extends string>({
  group,
  label,
  value,
  options,
  onChange,
  legend,
  sharesRow = false,
  hasPanel = false,
  activation = "auto",
  size = "compact",
  overflow = "select",
  className
}: DefaultTabsProps<T>) {
  const scrolls = overflow === "scroll";
  const { strip, select, fits } = useStripFits<HTMLDivElement>(
    options.map((o) => `${o.value}:${o.label}:${o.count ?? ""}`).join("|"),
    !scrolls
  );
  const touch = size === "touch";
  // A value matching no option leaves the first tab the Tab stop, so the strip is never
  // unreachable from the keyboard.
  const selectedIndex = Math.max(
    options.findIndex((o) => o.value === value),
    0
  );
  useSelectedInView(strip, selectedIndex, scrolls);

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const move = KEYS[e.key];
    if (!move) return;
    e.preventDefault();
    const next = move(index, options.length - 1);
    if (activation === "auto") onChange(options[next].value);
    // Found within THIS strip, not by id across the document: a group name is only unique per
    // strip, and one page may draw a group twice.
    strip.current?.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
  };

  const control = !fits ? (
    // Wider than the room it has: a select, one control naming the view.
    <select
      ref={select}
      id={tabId(group, value)}
      aria-label={label}
      aria-controls={hasPanel ? panelId(group) : undefined}
      value={value}
      onChange={(e) => onChange(e.target.value as T)}
      className={cx(touch ? ui.selectTouch : ui.select, "oxy-ui-default-tabs__select", className)}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {optionText(o.label, o.trailing === undefined ? o.count : undefined)}
        </option>
      ))}
    </select>
  ) : (
    <div
      ref={strip}
      role='tablist'
      aria-label={label}
      className={cx(
        ui.segmentRow,
        "oxy-ui-default-tabs",
        touch && "oxy-ui-default-tabs--touch",
        scrolls && "oxy-ui-default-tabs--scroll",
        className
      )}
    >
      {options.map((option, index) => {
        const on = option.value === value;
        return (
          <button
            key={option.value}
            id={tabId(group, option.value)}
            type='button'
            role='tab'
            aria-selected={on}
            aria-controls={hasPanel ? panelId(group) : undefined}
            tabIndex={index === selectedIndex ? 0 : -1}
            title={option.title}
            onClick={() => onChange(option.value)}
            onKeyDown={(e) => onKeyDown(e, index)}
            className={cx(
              ui.segment,
              "oxy-ui-default-tabs__tab",
              sharesRow && ui.segmentSharesRow,
              on ? ui.segmentOn : ui.segmentOff
            )}
          >
            {option.icon && <span className='oxy-ui-default-tabs__icon'>{option.icon}</span>}
            {option.label}
            {option.trailing ?? <TabCount option={option} on={on} />}
          </button>
        );
      })}
    </div>
  );

  if (!legend) return control;
  return (
    <div className='oxy-ui-default-tabs-row'>
      {control}
      {legend}
    </div>
  );
}

/** The tones a count shouts (the tone rule, decision 31): the rest read as neutral. */
const LOUD: ReadonlySet<StatusTone> = new Set(["error", "warning"]);

function TabCount<T extends string>({ option, on }: { option: TabOption<T>; on: boolean }) {
  if (typeof option.count !== "number") return null;
  const tone =
    option.countTone && LOUD.has(option.countTone)
      ? chipTone[option.countTone]
      : on
        ? ui.tabCountNeutralOn
        : ui.tabCountNeutral;
  return (
    <span className={cx(ui.tabCount, "oxy-ui-default-tabs__count", tone)}>{option.count}</span>
  );
}

export interface TabPanelProps {
  group: string;
  /** The selected value: its tab labels the panel. */
  value: string;
  /** Whether a tab labels this panel. False where the screens are links (`ParentTabs`), which
   *  own no panel: the body is then a plain block, not a `tabpanel` pointing at a missing tab. */
  labelled?: boolean;
  className?: string;
  children: ReactNode;
}

/** The one panel a `hasPanel` strip's tabs point at; labelled by the selected tab. */
export function TabPanel({ group, value, labelled = true, className, children }: TabPanelProps) {
  if (!labelled)
    return (
      <div data-screen={group} className={className}>
        {children}
      </div>
    );
  return (
    <div
      role='tabpanel'
      id={panelId(group)}
      aria-labelledby={tabId(group, value)}
      className={className}
    >
      {children}
    </div>
  );
}
