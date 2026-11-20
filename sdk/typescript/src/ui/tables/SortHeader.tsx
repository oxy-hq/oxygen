// Started from oxy-hq/reference-app@3caad1f `src/components/SortHeader.tsx` (INVENTORY section 3:
// all six copies put `aria-sort` on the header and a <button> inside it; supply-chain's is the
// closest and differs only in comments). Kept: the props, the three-state cycle (`nextSort`), the
// title that says what the NEXT press does, the arrow drawn hidden so the heading never jumps,
// and the shared focus ring. New: the props of the <th> it draws, and a 44px reach on a touch
// screen (decision 16) that grows the press area, not the heading.
//
// A column heading you can sort by. Three things it has to get right, none of them about sorting:
//
//   it is a BUTTON     inside the <th>, not a click handler on the cell, so it is reachable by
//                      keyboard and announced as pressable.
//   `aria-sort`        on the <th>, which is how a screen reader says which column the table is
//                      ordered by and which way. Nothing else on screen carries that.
//   the arrow is not   the whole signal: the direction is in `aria-sort` and in the title. An
//                      arrow alone is a ten-pixel glyph doing the work of a sentence.
//
// The state is the caller's: hold a `SortState | null`, and in `onSort` call
// `nextSort(sort, key, firstDir)` (./tableSort), then `sortRows`.

import type { ComponentPropsWithRef, ReactNode } from "react";
import { cx } from "../cx";
import { CaretDownIcon } from "../icons";
import { ui } from "../recipes";
import { CaretUpIcon } from "./icons";
import type { SortDir, SortState } from "./tableSort";

export interface SortHeaderProps<K extends string>
  extends Omit<ComponentPropsWithRef<"th">, "children" | "scope" | "aria-sort" | "align"> {
  sortKey: K;
  /** For the accessible title: "Sort by food cost". Not rendered. */
  label: string;
  /** Which way this column goes on the first press. Dollars want the biggest first; a rating
   *  wants the worst first. */
  firstDir: SortDir;
  sort: SortState<K> | null;
  onSort: (key: K, firstDir: SortDir) => void;
  align?: "left" | "right";
  /** The heading as it reads on screen, which is often more than the label. */
  children: ReactNode;
}

const ARIA_SORT = { asc: "ascending", desc: "descending" } as const;

export function SortHeader<K extends string>({
  sortKey,
  label,
  firstDir,
  sort,
  onSort,
  align = "left",
  className,
  children,
  ...rest
}: SortHeaderProps<K>) {
  const active = sort?.key === sortKey ? sort.dir : null;
  // Says what the NEXT press does, which is what a reader wants to know and an icon cannot say.
  const title =
    active === null
      ? `Sort by ${label}`
      : active === firstDir
        ? `Sort by ${label}, the other way`
        : "Back to the default order";
  const Caret = active === "desc" ? CaretDownIcon : CaretUpIcon;
  return (
    <th
      {...rest}
      scope='col'
      // `aria-sort` belongs on the header cell, not on the button.
      aria-sort={active ? ARIA_SORT[active] : "none"}
      className={cx(
        "oxy-ui-sort-header",
        align === "right" && "oxy-ui-sort-header--right",
        ui.eyebrow,
        className
      )}
    >
      <button
        type='button'
        title={title}
        className={cx("oxy-ui-sort-header__button", ui.focusRing)}
        onClick={() => onSort(sortKey, firstDir)}
      >
        <span className='oxy-ui-sort-header__text'>{children}</span>
        {/* Drawn and hidden rather than omitted, so the heading does not jump sideways when the
            column becomes the sorted one; it shows faintly on hover, which is what tells a
            reader the heading is pressable before they press it. */}
        <span className='oxy-ui-sort-header__arrow' data-on={active ? "" : undefined}>
          <Caret className='oxy-ui-sort-header__caret' />
        </span>
      </button>
    </th>
  );
}
