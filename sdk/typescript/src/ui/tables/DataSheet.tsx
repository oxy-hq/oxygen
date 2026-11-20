// Started from oxy-hq/reference-app@3caad1f `src/components/DataSheet.tsx` (INVENTORY section 3:
// the only copy; BMG's reorder sheet, its origin, is hand CSS). Kept whole: identity columns pinned
// left at fixed widths, the row's one action pinned right, the header held on the vertical
// scroll with its corners held on both axes, band / header / rows on the fixed sheet heights,
// column groups that fold to a stub that still names them, `rowTone` as a 3px stripe on the
// pinned first cell, under a 56.25rem viewport only the first column pinned, and the box as a Tab
// stop and a named region only while it overflows (`useScrollFocusable` from ../focus, the focus
// group's port of the same hook). New: its Tailwind classes are `.oxy-ui-data-sheet*` rules on
// theme tokens (tables.css), and it takes a `className`.
//
// The wide, all-figures table a whole screen is spent reading. Why not PanelTable: a PanelTable
// fits its frame; a sheet is wider than the window by design, so the parts a reader needs on
// every row (its name, its action) have to survive the scroll the rest of it takes.
//
// No phone form of its own: at phone width it already holds only its first column and scrolls
// the rest both ways inside its box, which is how it is read there; a screen that wants cards on
// a phone switches to the phone kit itself.

import type { CSSProperties, ReactNode } from "react";
import { cx } from "../cx";
import { useScrollFocusable } from "../focus";
import { ui } from "../recipes";
import type { StatusTone } from "../tone";

export interface SheetColumn<R> {
  key: string;
  label: string;
  /** Right-set, tabular figures. */
  numeric?: boolean;
  render: (row: R) => ReactNode;
}

export interface PinnedColumn<R> extends SheetColumn<R> {
  /** Fixed: a pinned column's offset is the sum of the widths before it, and an offset cannot be
   *  measured from content. Any CSS length ("9rem"). */
  width: string;
}

export interface SheetGroup<R> {
  id: string;
  label: string;
  /** The stub's name when the group is folded. */
  short: string;
  columns: SheetColumn<R>[];
}

export interface DataSheetProps<R> {
  /** The table's accessible name, and the scroll box's while it scrolls. */
  label: string;
  rows: readonly R[];
  rowKey: (row: R) => string;
  pinned: PinnedColumn<R>[];
  groups: SheetGroup<R>[];
  /** Group ids folded to a stub. The switch (a ToggleChipGroup above) and its memory are the
   *  caller's. */
  hidden?: ReadonlySet<string>;
  /** One column pinned to the right edge: the row's action. */
  action?: { label: string; render: (row: R) => ReactNode };
  /** The row's status, as a stripe down its first cell. */
  rowTone?: (row: R) => StatusTone | undefined;
  /** Any CSS length. Default 30rem. */
  maxHeight?: string;
  className?: string;
}

export function DataSheet<R>({
  label,
  rows,
  rowKey,
  pinned,
  groups,
  hidden,
  action,
  rowTone,
  maxHeight = "30rem",
  className
}: DataSheetProps<R>) {
  const folded = (g: SheetGroup<R>) => hidden?.has(g.id) ?? false;
  const offsets = pinned.map((_, i) =>
    i === 0
      ? "0px"
      : `calc(${pinned
          .slice(0, i)
          .map((p) => p.width)
          .join(" + ")})`
  );
  const pinStyle = (i: number) =>
    ({
      "--oxy-ui-pin-left": offsets[i],
      width: pinned[i].width,
      minWidth: pinned[i].width,
      maxWidth: pinned[i].width
    }) as CSSProperties;
  const lastPinned = pinned.length - 1;
  const box = useScrollFocusable<HTMLDivElement>();
  // Only while it scrolls: a box that fits is no stop at all, and a region with nothing to scroll
  // is a landmark with nothing in it.
  const region = box.scrolls ? { role: "region", "aria-label": label } : {};
  const pin = (i: number) =>
    cx(
      "oxy-ui-data-sheet__pin",
      i === 0 && "oxy-ui-data-sheet__pin--first",
      i === lastPinned && "oxy-ui-data-sheet__pin--last"
    );

  return (
    <div
      {...region}
      ref={box.ref}
      tabIndex={box.tabIndex}
      className={cx("oxy-ui-data-sheet", ui.focusRing, className)}
      style={{ maxHeight }}
    >
      <table aria-label={label} className='oxy-ui-data-sheet__table'>
        <thead>
          <tr className={ui.sheetBand}>
            <th
              colSpan={pinned.length}
              className={cx(ui.text3xs, "oxy-ui-data-sheet__band oxy-ui-data-sheet__band--corner")}
            />
            {groups.map((g) =>
              folded(g) ? (
                <th
                  key={g.id}
                  className={cx(
                    ui.text3xs,
                    "oxy-ui-data-sheet__band oxy-ui-data-sheet__band--folded"
                  )}
                >
                  {g.short}
                </th>
              ) : (
                <th
                  key={g.id}
                  colSpan={g.columns.length}
                  className={cx(ui.text3xs, "oxy-ui-data-sheet__band")}
                >
                  {g.label}
                </th>
              )
            )}
            {action && (
              <th
                className={cx(
                  ui.text3xs,
                  "oxy-ui-data-sheet__band oxy-ui-data-sheet__band--action"
                )}
              />
            )}
          </tr>
          <tr className={ui.sheetHeader}>
            {pinned.map((p, i) => (
              <th
                key={p.key}
                scope='col'
                style={pinStyle(i)}
                className={cx(
                  ui.text2xs,
                  "oxy-ui-data-sheet__head",
                  pin(i),
                  p.numeric && "oxy-ui-data-sheet__head--num"
                )}
              >
                {p.label}
              </th>
            ))}
            {groups.map((g) =>
              folded(g) ? (
                <th
                  key={g.id}
                  className={cx(ui.text2xs, "oxy-ui-data-sheet__head")}
                  aria-label={`${g.label} (folded)`}
                />
              ) : (
                g.columns.map((c) => (
                  <th
                    key={`${g.id}-${c.key}`}
                    scope='col'
                    className={cx(
                      ui.text2xs,
                      "oxy-ui-data-sheet__head",
                      c.numeric && "oxy-ui-data-sheet__head--num"
                    )}
                  >
                    {c.label}
                  </th>
                ))
              )
            )}
            {action && (
              <th
                scope='col'
                className={cx(
                  ui.text2xs,
                  "oxy-ui-data-sheet__head oxy-ui-data-sheet__head--num oxy-ui-data-sheet__action"
                )}
              >
                {action.label}
              </th>
            )}
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const tone = rowTone?.(r);
            return (
              <tr key={rowKey(r)} className={ui.sheetRow}>
                {pinned.map((p, i) => {
                  const Cell = i === 0 ? "th" : "td";
                  return (
                    <Cell
                      key={p.key}
                      scope={i === 0 ? "row" : undefined}
                      style={pinStyle(i)}
                      data-tone={i === 0 && tone ? tone : undefined}
                      className={cx(
                        ui.cell,
                        "oxy-ui-data-sheet__cell oxy-ui-data-sheet__cell--cut",
                        pin(i),
                        i === 0 && "oxy-ui-data-sheet__name",
                        p.numeric && "oxy-ui-data-sheet__cell--num"
                      )}
                    >
                      {p.render(r)}
                    </Cell>
                  );
                })}
                {groups.map((g) =>
                  folded(g) ? (
                    <td
                      key={g.id}
                      className={cx(
                        ui.cell,
                        "oxy-ui-data-sheet__cell oxy-ui-data-sheet__cell--folded"
                      )}
                    />
                  ) : (
                    g.columns.map((c) => (
                      <td
                        key={`${g.id}-${c.key}`}
                        className={cx(
                          ui.cell,
                          "oxy-ui-data-sheet__cell",
                          c.numeric && "oxy-ui-data-sheet__cell--num"
                        )}
                      >
                        {c.render(r)}
                      </td>
                    ))
                  )
                )}
                {action && (
                  // A 32px control: the cell's own padding round it would push the row past
                  // the sheet's row height, so the action cell takes less.
                  <td className={cx(ui.cell, "oxy-ui-data-sheet__cell oxy-ui-data-sheet__action")}>
                    {action.render(r)}
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
