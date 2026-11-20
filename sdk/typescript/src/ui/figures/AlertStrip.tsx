// Started from pokehouse-oxy `apps/supply-chain/src/components/AlertStrip.tsx` (INVENTORY
// section 2: the only copy whose alerts are real links, `href` + a handler, so ⌘-click opens a
// new tab), itself the reference's (oxy-hq/reference-app@3caad1f `src/components/AlertStrip.tsx`).
// Taken with it, from Store Ops' copy (`apps/store-ops/src/chrome/AlertStrip.tsx`): an alert that
// only informs (no `href`, no `onSelect`) is a plain callout, `allClear` takes any node, and the
// strip takes a `className`. The reference's `onSelect` (a button door) stays for an alert that
// leads to a state rather than a URL. New in the SDK: decision 37's padding and the end fade, which
// lived only in the apps' `index.css` `.scroll-row` rules (INVENTORY surprise 4), are figures.css's.
//
// What needs attention before a single row is read, and the way to it.
//
// **Every alert is a door, not a sign.** Each counts some rows and takes the reader to exactly
// those: given `href`, it is a link (a plain left-click calls `onNavigate` in place; any other
// click is the browser's); given `onSelect`, a button. Either wears a caret. An alert with
// neither is a plain callout: no caret, no hover, not a control.
//
// **Status callouts, one line.** Each alert is the `callout` recipe in its tone. The strip never
// wraps: on a narrow screen it scrolls sideways under its label, which holds on the left, and
// each edge fades while there is more past it. Wrapping would push down the rows it points at.
//
// **Nothing to say is said.** With no alerts the strip shows its all-clear in the success tone,
// so "nothing is wrong" reads as an answer, not as a strip that failed to load. No live region:
// the strip is the screen's standing summary, read where it stands (Store Ops' note).
//
// **Decision 37.** The strip is a card's ground on a grey page, so it takes a tile's inner
// padding at both ends: the label starts where a tile's label does (its 1px frame and the 12px
// of `paddingSm`), and the last callout ends as far from the right edge. An app whose strip must
// not move (Store Ops on an enrolled store tablet) sets `--oxy-ui-alert-strip-inset: 0px`.

import type { MouseEvent, ReactNode } from "react";
import { cx } from "../cx";
import { CaretRightIcon } from "../icons";
import { ui } from "../recipes";
import type { StatusTone } from "../tone";

export interface AlertStripAlert {
  tone: StatusTone;
  /** The sentence, with its count: "3 reports not emailed". */
  label: ReactNode;
  /** The door's accessible name, where the label alone does not say where it leads. */
  ariaLabel?: string;
  /** Where the alert leads, as a URL: the alert is a link. */
  href?: string;
  /** A plain left-click on a link alert, followed in place (the default is prevented first);
   *  ⌘-, ctrl-, shift- and middle-clicks stay the browser's. */
  onNavigate?: (event: MouseEvent<HTMLAnchorElement>) => void;
  /** Supply Chain's name for `onNavigate`, kept so its move is an import swap. */
  onFollow?: (event: MouseEvent<HTMLAnchorElement>) => void;
  /** Where the alert leads, as a state (a tab, a filter): the alert is a button. */
  onSelect?: () => void;
}

export interface AlertStripProps {
  alerts: readonly AlertStripAlert[];
  /** The eyebrow naming the strip, and the group's accessible name. */
  label?: string;
  /** What the strip says when there is nothing to say. */
  allClear?: ReactNode;
  className?: string;
}

/** A click the page should follow in place: the main button, no modifier key. */
export function isPlainClick(event: MouseEvent): boolean {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

const callout = (tone: StatusTone, door: boolean) =>
  cx(
    ui.callout,
    ui.calloutTone[tone],
    "oxy-ui-alert-strip__alert",
    door && "oxy-ui-alert-strip__door"
  );

function Door({ alert }: { alert: AlertStripAlert }) {
  const caret = <CaretRightIcon className={cx(ui.iconInline, "oxy-ui-alert-strip__caret")} />;
  const follow = alert.onNavigate ?? alert.onFollow;
  if (alert.href !== undefined) {
    return (
      <a
        href={alert.href}
        aria-label={alert.ariaLabel}
        className={cx(callout(alert.tone, true), ui.focusRing)}
        data-tone={alert.tone}
        onClick={(event) => {
          if (!follow || !isPlainClick(event)) return;
          event.preventDefault();
          follow(event);
        }}
      >
        {alert.label}
        {caret}
      </a>
    );
  }
  if (alert.onSelect) {
    return (
      <button
        type='button'
        aria-label={alert.ariaLabel}
        className={cx(callout(alert.tone, true), ui.focusRing)}
        data-tone={alert.tone}
        onClick={alert.onSelect}
      >
        {alert.label}
        {caret}
      </button>
    );
  }
  return (
    <span className={callout(alert.tone, false)} data-tone={alert.tone}>
      {alert.label}
    </span>
  );
}

export function AlertStrip({
  alerts,
  label = "Attention",
  allClear = "Nothing needs attention",
  className
}: AlertStripProps) {
  return (
    // A strip of links and words, not form controls: a <fieldset> and its <legend> would say
    // "form" and draw a border; the group is named by its eyebrow, as the reference's is.
    // biome-ignore lint/a11y/useSemanticElements: see above
    <div role='group' aria-label={label} className={cx("oxy-ui-alert-strip", className)}>
      <span className={cx(ui.eyebrow, "oxy-ui-alert-strip__label")}>{label}</span>
      {alerts.length === 0 ? (
        <span className={callout("success", false)} data-tone='success' data-all-clear=''>
          {allClear}
        </span>
      ) : (
        // biome-ignore lint/suspicious/noArrayIndexKey: alerts carry no id, and the strip is rebuilt whole
        alerts.map((alert, i) => <Door key={i} alert={alert} />)
      )}
    </div>
  );
}
