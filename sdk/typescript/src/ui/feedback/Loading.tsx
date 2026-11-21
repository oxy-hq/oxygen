// Started from pokehouse-oxy `apps/supply-chain/src/components/Loading.tsx` (INVENTORY section 2:
// the reference's `LoadingBar` and `SkeletonRows` plus the retry-focus target, diff 54, the closest
// of the four focus-aware copies), with Marketing's `Loading` (the sentence over the skeleton, so
// the wait is announced and not only drawn) and Bookkeeping's `KitLoadingText` (the same target in
// a phone kit row). Kept: the names and props, `role="status"`, the bar's honest count and its
// visible sliver at zero, the fixed widths, `tabIndex=-1` holding focus after Try again
// (focus/useRetryFocus.ts; never a Tab stop, no ring). Changed: the progress type is
// `LoadingProgress` (the copies' `Progress` is a name too common for one entry's namespace).
//
// What a screen says while it waits. A static "Comparing stores…" for five seconds is
// indistinguishable from a hung screen, so the wait is made visible two ways:
//
//   the skeleton   answers "is this going to be a table?": it holds the shape the content will
//                  take, so the page looks unfinished rather than empty;
//   the progress   answers "is it stuck?": a determinate bar over the parts that have actually
//                  come back. No spinner (it says "waiting" alike at one second and at thirty) and
//                  no percentage (it reads as a time estimate, and the parts are uneven).

import { type ReactNode, useRef } from "react";
import { cx } from "../cx";
import { kitRow, useHoldFocusDuringRetry } from "../focus/useRetryFocus";
import { ui } from "../recipes";

/** Parts resolved out of parts issued. */
export interface LoadingProgress {
  ready: number;
  total: number;
}

export interface LoadingBarProps {
  /** What is being waited for: "Comparing stores…". */
  label: string;
  /** Parts resolved out of parts issued. Default one read, not back yet (`{ ready: 0, total: 1 }`),
   *  for a screen that waits on a single read (2.31.0; Warehouse's ten bars). */
  progress?: LoadingProgress;
}

/** One read, not back yet. */
const ONE_READ: LoadingProgress = { ready: 0, total: 1 };

/** A determinate bar and the sentence that says what is being read. It never draws empty: a bar
 *  at zero reads as broken, so it starts at a visible sliver. */
export function LoadingBar({ label, progress = ONE_READ }: LoadingBarProps) {
  const pct = progress.total > 0 ? (progress.ready / progress.total) * 100 : 0;
  const root = useRef<HTMLDivElement>(null);
  useHoldFocusDuringRetry(root);
  return (
    <div
      ref={root}
      tabIndex={-1}
      data-retry-focus=''
      className='oxy-ui-loading-bar'
      role='status'
      aria-live='polite'
      // The name carries the count: the bar is decorative to a screen reader, and "loading" on
      // its own is no better than the static string this replaced.
      aria-label={`${label} — ${progress.ready} of ${progress.total} parts ready`}
    >
      <p className='oxy-ui-loading-bar__label'>{label}</p>
      {/* Square, track and fill: a progress bar is a measured length. */}
      <div className='oxy-ui-loading-bar__track'>
        <div className='oxy-ui-loading-bar__fill' style={{ width: `${Math.max(4, pct)}%` }} />
      </div>
    </div>
  );
}

/** Each bar's width from a fixed pattern, not at random: random widths change every render and
 *  make the block twitch while it waits. */
const BAR_WIDTHS = ["4rem", "2.5rem", "3.5rem", "3rem", "4rem", "2.5rem", "3rem", "3.5rem"];

export interface SkeletonRowsProps {
  /** Rows to draw. Default 6. */
  rows?: number;
  /** Columns after the name, passed in so the skeleton cannot drift from the table it stands
   *  in for (the wrong count makes the layout jump when the real thing arrives). Default 6. */
  cols?: number;
}

/** The table's own shape, in grey, hidden from assistive tech: the status line says the words. */
export function SkeletonRows({ rows = 6, cols = 6 }: SkeletonRowsProps) {
  return (
    <div aria-hidden='true' className='oxy-ui-skeleton'>
      {Array.from({ length: rows }, (_, r) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: a fixed grid with no identity of its own.
        <div key={r} className='oxy-ui-skeleton__row'>
          <div className='oxy-ui-skeleton__bar oxy-ui-skeleton__bar--name' />
          {Array.from({ length: cols }, (_, c) => (
            <div
              // biome-ignore lint/suspicious/noArrayIndexKey: as above.
              key={c}
              className='oxy-ui-skeleton__bar'
              style={{ width: BAR_WIDTHS[(r + c) % BAR_WIDTHS.length] }}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

export interface LoadingProps {
  /** What is being read, announced: "Loading this week's ads…". */
  label: string;
  rows?: number;
  cols?: number;
  className?: string;
}

/** A screen, or a part of one, waiting on a read: the announced sentence over the skeleton in
 *  the shape the content will take, on a card. */
export function Loading({ label, rows = 6, cols = 6, className }: LoadingProps) {
  const line = useRef<HTMLParagraphElement>(null);
  useHoldFocusDuringRetry(line);
  return (
    <section className={cx(ui.card, "oxy-ui-loading", className)}>
      <p
        ref={line}
        tabIndex={-1}
        data-retry-focus=''
        role='status'
        aria-live='polite'
        className='oxy-ui-loading__label'
      >
        {label}
      </p>
      <SkeletonRows rows={rows} cols={cols} />
    </section>
  );
}

export interface LoadingLineProps {
  className?: string;
  children: ReactNode;
}

/** A one-line loading status inside a panel (an item's store orders), holding focus after Try
 *  again as the bar does. */
export function LoadingLine({ className, children }: LoadingLineProps) {
  const line = useRef<HTMLParagraphElement>(null);
  useHoldFocusDuringRetry(line);
  return (
    <p
      ref={line}
      tabIndex={-1}
      data-retry-focus=''
      role='status'
      aria-live='polite'
      className={cx(ui.note, "oxy-ui-loading-line", className)}
    >
      {children}
    </p>
  );
}

/** The words of a phone kit row that says a re-read is running (a `PhoneRow`, which takes no
 *  ref): the row around the span holds focus after Try again. */
export function KitLoadingText({ children }: { children: ReactNode }) {
  const span = useRef<HTMLSpanElement>(null);
  useHoldFocusDuringRetry(span, kitRow);
  return <span ref={span}>{children}</span>;
}
