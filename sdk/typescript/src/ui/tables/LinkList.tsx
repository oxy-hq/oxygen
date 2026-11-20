// Started from oxy-hq/reference-app@3caad1f `src/views/WarehouseReports.tsx:204-268` (ReportList,
// the "Link list" variant on the pattern page's PanelTable row, `PatternSpecimens.tsx:693-697`; it
// has no copy elsewhere). Kept: a list of links that reads as a table, every row one 44px link on
// one shared column track, the current row in the selected fill (`ui.rowState`, so hover never
// strips it) and `aria-current="page"`, and the columns that drop by a CONTAINER query, on the
// list's own width rather than the window's (a side panel opening narrows the list, not the
// window). New: the tracks, the dropped columns and the links come in as props
// (the reference's were Warehouse's receipt columns), the link is router-free (`href` +
// `onNavigate`), and the focus ring is drawn inside the row, which the card clips.
//
// Not a <table>: each row goes somewhere, and a list of links is what a screen reader should
// announce. For rows that open in place, PanelTable's `onRowOpen`.

import type { CSSProperties, MouseEvent, ReactNode } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";

export interface LinkListColumn<T> {
  /** The column's grid track: "5rem", "minmax(0,1fr)". */
  width: string;
  cell: (row: T) => ReactNode;
  align?: "left" | "right";
  /** Dropped while the list is narrower than 48rem (the reference's `@3xl`): a column the
   *  detail beside the list repeats, so it goes rather than squeezes. */
  wide?: boolean;
  /** One line, cut with an ellipsis, rather than wrapping. */
  truncate?: boolean;
  className?: string;
}

export interface LinkListProps<T> {
  rows: readonly T[];
  rowKey: (row: T) => string;
  columns: LinkListColumn<T>[];
  href: (row: T) => string;
  /** Router-free: called on a plain left press instead of following the href (a modified press,
   *  to open a new tab, still follows it). */
  onNavigate?: (href: string, row: T) => void;
  /** The row the page is showing: the selected fill and `aria-current="page"`. */
  isCurrent?: (row: T) => boolean;
  /** The list's accessible name. */
  label?: string;
  className?: string;
}

const plain = (event: MouseEvent) =>
  event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;

export function LinkList<T>({
  rows,
  rowKey,
  columns,
  href,
  onNavigate,
  isCurrent,
  label,
  className
}: LinkListProps<T>) {
  // The two track lists, as custom properties the stylesheet picks between by container width.
  const tracks = {
    "--oxy-ui-link-list-full": columns.map((c) => c.width).join(" "),
    "--oxy-ui-link-list-compact": columns
      .filter((c) => !c.wide)
      .map((c) => c.width)
      .join(" ")
  } as CSSProperties;
  return (
    <ul className={cx(ui.card, "oxy-ui-link-list", className)} style={tracks} aria-label={label}>
      {rows.map((row) => {
        const current = isCurrent?.(row) ?? false;
        const to = href(row);
        return (
          <li key={rowKey(row)} className='oxy-ui-link-list__item'>
            <a
              href={to}
              aria-current={current ? "page" : undefined}
              className={cx("oxy-ui-link-list__row", ui.focusRingInset, ui.rowState(current))}
              onClick={(event) => {
                if (!onNavigate || !plain(event)) return;
                event.preventDefault();
                onNavigate(to, row);
              }}
            >
              {columns.map((c, i) => (
                <span
                  key={i}
                  className={cx(
                    "oxy-ui-link-list__cell",
                    c.align === "right" && "oxy-ui-link-list__cell--num",
                    c.truncate && "oxy-ui-link-list__cell--truncate",
                    c.wide && "oxy-ui-link-list__cell--wide",
                    c.className
                  )}
                >
                  {c.cell(row)}
                </span>
              ))}
            </a>
          </li>
        );
      })}
    </ul>
  );
}
