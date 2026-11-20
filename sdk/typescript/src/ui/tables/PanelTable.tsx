// Started from oxy-hq/reference-app@3caad1f `src/components/PanelTable.tsx` (INVENTORY section 3:
// the reference's 500 lines over its two copies, Watchlist's and quadrogen's, which hold only its
// skeleton). The SDK keeps the skeleton, its `render` (lines 82-143): an eyebrow caption, an
// optional lead above and footnote below in the note ink, 11px caps headings, rows on the subtle
// rule with none under the last, names left and figures right, the row's name as
// `<th scope="row">` unless `rowHeader` is off. The four Poke House kinds it wraps (cohort,
// discounts, voids, platforms) stay in the app as column definitions passed in.
//
// New, from the table variants the reference's pattern page shows beside it
// (`views/PatternSpecimens.tsx:637-698`), so each is one frame instead of a hand-drawn table:
//   row actions   `actions` (Payroll's Post, `BookkeepingPayroll.tsx` GroupTable) and
//                 `onRowOpen` + `isSelected` (the row opens a detail panel, `ui.rowState`, so
//                 hover never strips the selected fill).
//   totals        each column's `total` and `totalLabel`, a muted semibold closing row
//                 (`Bookkeeping.tsx` VendorTable), drawn as a <tfoot>.
//   expandable    `subRows`: a row opens into child rows on the parent's own columns, so the parent
//                 row is their total (`BookkeepingSalesTax.tsx` CompanySection), one open at a time.
// The reference opened those rows with a click handler on the <tr>, which no keyboard reaches;
// here the row's name is a <button> (`aria-expanded` on an expandable row, `aria-current` on the
// selected one), and a press anywhere on the row still works for a mouse.
//
// The compact table under a heading inside a panel or a dialog, about one record. Not for the
// wide grids a whole screen is spent reading (DataSheet), and with no sideways scroll of its own:
// keep it to columns that fit a panel, or put a wider table in TableScroll.
//
// No phone form: it takes its panel's width, and on a phone that panel is a phone sheet whose
// width the same few columns fit, so there is nothing for it to switch to.

import {
  type ComponentPropsWithRef,
  type MouseEvent,
  type ReactNode,
  useId,
  useState
} from "react";
import { cx } from "../cx";
import { CaretDownIcon, CaretRightIcon } from "../icons";
import { ui } from "../recipes";

export interface PanelColumn<T> {
  header: ReactNode;
  /** Figures go right, names go left. Default left. */
  align?: "left" | "right";
  /** Classes for this column's cells; a function when the tone depends on the row. */
  className?: string | ((row: T) => string | undefined);
  cell: (row: T) => ReactNode;
  /** This column's figure in the totals row (with `totalLabel`). */
  total?: ReactNode;
}

export interface PanelTableProps<T> extends Omit<ComponentPropsWithRef<"div">, "children"> {
  /** The eyebrow over the table, and the table's accessible name. */
  caption?: ReactNode;
  /** The table's name when there is no caption. */
  label?: string;
  columns: PanelColumn<T>[];
  rows: readonly T[];
  rowKey: (row: T, index: number) => string;
  /** A sentence ABOVE the table: what a reader should take from it first. */
  lead?: ReactNode;
  /** A sentence below: usually the reconciliation of a capped list. */
  footnote?: ReactNode;
  /** Per-row emphasis (a cohort's subject row takes its status tint). */
  rowClassName?: (row: T) => string | undefined;
  /** First cell as `<th scope="row">`: on for rows that are NAMED (a discount, an item). Off for
   *  rows that are records in a comparison, whose subject is marked by tint. Default on. */
  rowHeader?: boolean;
  /** The row opens something (a detail panel): a press anywhere on it, or its name's button. */
  onRowOpen?: (row: T) => void;
  /** The open row takes the selected fill, and its name's button says `aria-current`. */
  isSelected?: (row: T) => boolean;
  /** A cell of actions closing each row. A press in it never opens the row. */
  actions?: (row: T) => ReactNode;
  /** The actions column's heading, for assistive tech only. Default "Actions". */
  actionsLabel?: string;
  /** Draws the totals row: this in its first cell ("Total (12 stores)"), each column's `total`
   *  after it. */
  totalLabel?: ReactNode;
  /** The rows a row opens into, drawn on the same columns. A row with none does not open. */
  subRows?: (row: T) => readonly T[] | null | undefined;
  /** The open row's key (one at a time). Leave unset to let the table hold it. */
  expanded?: string | null;
  onExpandedChange?: (key: string | null) => void;
}

/** A press inside the actions cell is the action's, not the row's. */
const keepToCell = (event: MouseEvent) => event.stopPropagation();

export function PanelTable<T>({
  caption,
  label,
  columns,
  rows,
  rowKey,
  lead,
  footnote,
  rowClassName,
  rowHeader = true,
  onRowOpen,
  isSelected,
  actions,
  actionsLabel = "Actions",
  totalLabel,
  subRows,
  expanded,
  onExpandedChange,
  className,
  ...rest
}: PanelTableProps<T>) {
  const captionId = useId();
  const [held, setHeld] = useState<string | null>(null);
  const open = expanded !== undefined ? expanded : held;
  const setOpen = (key: string | null) => {
    if (expanded === undefined) setHeld(key);
    onExpandedChange?.(key);
  };
  const hasCaption = caption !== undefined && caption !== null && caption !== false;
  const totals = totalLabel !== undefined;

  const cells = (row: T, opener: ReactNode | null, sub: boolean) =>
    columns.map((c, j) => {
      const own = typeof c.className === "function" ? c.className(row) : c.className;
      const content = c.cell(row);
      const named = j === 0 && rowHeader;
      const Cell = named ? "th" : "td";
      return (
        <Cell
          key={j}
          scope={named ? "row" : undefined}
          className={cx(
            "oxy-ui-panel-table__cell",
            named && "oxy-ui-panel-table__cell--name",
            c.align === "right" && "oxy-ui-panel-table__cell--num",
            sub && j === 0 && "oxy-ui-panel-table__cell--sub",
            own
          )}
        >
          {j === 0 && opener ? opener : content}
        </Cell>
      );
    });

  const body = rows.flatMap((row, i) => {
    const key = rowKey(row, i);
    const kids = subRows?.(row) ?? [];
    const expandable = kids.length > 0;
    const isOpen = expandable && open === key;
    const selected = isSelected?.(row) ?? false;
    const interactive = expandable || onRowOpen !== undefined;
    const press = () => {
      if (expandable) setOpen(isOpen ? null : key);
      else onRowOpen?.(row);
    };
    // The keyboard's way in: the row's name as a button with no handler of its own, so its press
    // bubbles to the row's, one press either way.
    const opener = interactive ? (
      <button
        key='open'
        type='button'
        className={cx("oxy-ui-panel-table__open", ui.focusRing)}
        aria-expanded={expandable ? isOpen : undefined}
        aria-current={!expandable && selected ? "true" : undefined}
      >
        {expandable && (
          <span className='oxy-ui-panel-table__caret'>
            {isOpen ? (
              <CaretDownIcon className={ui.iconInline} />
            ) : (
              <CaretRightIcon className={ui.iconInline} />
            )}
          </span>
        )}
        {columns[0]?.cell(row)}
      </button>
    ) : null;
    const line = (
      <tr
        key={key}
        className={cx(
          "oxy-ui-panel-table__row",
          interactive && "oxy-ui-panel-table__row--open",
          interactive ? ui.rowState(selected || isOpen) : selected && ui.rowSelected,
          rowClassName?.(row)
        )}
        onClick={interactive ? press : undefined}
      >
        {cells(row, opener, false)}
        {actions && (
          <td
            className='oxy-ui-panel-table__cell oxy-ui-panel-table__cell--actions'
            onClick={keepToCell}
          >
            {actions(row)}
          </td>
        )}
      </tr>
    );
    if (!isOpen) return [line];
    return [
      line,
      ...kids.map((kid, j) => (
        <tr
          key={`${key}/${rowKey(kid, j)}`}
          className='oxy-ui-panel-table__row oxy-ui-panel-table__sub-row'
          data-last={j === kids.length - 1 ? "" : undefined}
        >
          {cells(kid, null, true)}
          {actions && <td className='oxy-ui-panel-table__cell' />}
        </tr>
      ))
    ];
  });

  return (
    <div {...rest} className={cx("oxy-ui-panel-table", className)}>
      {hasCaption && (
        <h3 id={captionId} className={cx(ui.eyebrow, "oxy-ui-panel-table__caption")}>
          {caption}
        </h3>
      )}
      {lead && <p className={cx(ui.note, "oxy-ui-panel-table__note")}>{lead}</p>}
      <table
        className={cx("oxy-ui-panel-table__table", totals && "oxy-ui-panel-table__table--totals")}
        aria-labelledby={hasCaption ? captionId : undefined}
        aria-label={hasCaption ? undefined : label}
      >
        <thead>
          <tr className='oxy-ui-panel-table__head-row'>
            {columns.map((c, i) => (
              <th
                key={i}
                scope='col'
                className={cx(
                  ui.text2xs,
                  "oxy-ui-panel-table__head",
                  c.align === "right" && "oxy-ui-panel-table__head--num"
                )}
              >
                {c.header}
              </th>
            ))}
            {actions && (
              <th
                scope='col'
                className={cx(
                  ui.text2xs,
                  "oxy-ui-panel-table__head",
                  "oxy-ui-panel-table__head--actions"
                )}
              >
                <span className='oxy-ui-panel-table__hidden'>{actionsLabel}</span>
              </th>
            )}
          </tr>
        </thead>
        <tbody>{body}</tbody>
        {totals && (
          <tfoot>
            <tr className='oxy-ui-panel-table__totals'>
              {columns.map((c, j) =>
                j === 0 ? (
                  <th key={j} scope='row' className='oxy-ui-panel-table__cell'>
                    {totalLabel}
                  </th>
                ) : (
                  <td
                    key={j}
                    className={cx(
                      "oxy-ui-panel-table__cell",
                      c.align === "right" && "oxy-ui-panel-table__cell--num"
                    )}
                  >
                    {c.total}
                  </td>
                )
              )}
              {actions && <td className='oxy-ui-panel-table__cell' />}
            </tr>
          </tfoot>
        )}
      </table>
      {footnote && <p className={cx(ui.note, "oxy-ui-panel-table__note")}>{footnote}</p>}
    </div>
  );
}
