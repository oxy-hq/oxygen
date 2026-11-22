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
// No phone form by default: it takes its panel's width, and on a phone that panel is usually a
// phone sheet whose width the same few columns fit.
//
// 2.32.0 (the kept pieces, decision 99), each opt-in, so a table that passes none of it is drawn
// as before:
//   - `phoneCard`: on a phone layout (`usePhoneLayout`), each row is a card instead of a table row,
//     for a table of ten columns that would otherwise scroll sideways (Quadrogen's fleet of 120
//     plants, its `PhoneRows`). The card is the app's own, not the phone kit's: the tile's fill and
//     edge (`ui.tile`) at `paddingSm`; the title column on the first line at the table's type, the
//     `aside` column (a status) on that line's right with the chevron after it, on one baseline;
//     a one-line `subtitle` on its own line; then the figures as label over value, as many as
//     fit across (`5.5rem` each), a `phoneWide` column taking the card's whole width. No figure
//     cap. A row that opens is pressed anywhere on the card, and its title is the button the
//     keyboard reaches, as on the desk.
//   - `rowProps`: per-row attributes (`data-*`, `title`, `tabIndex`, key and click handlers) on
//     the desk row, after the table's own; its `className` joins `rowClassName`, and its `onClick`
//     is the row's when the table does not open rows itself. On a card the `data-*`, `aria-*` and
//     `className` go on the card and `onClick` opens it; `title`, `tabIndex` and `onKeyDown` stay
//     desk-only (no hover on a phone; the card's own button takes the keyboard).

import {
  type ComponentPropsWithRef,
  type HTMLAttributes,
  type ReactNode,
  useId,
  useState
} from "react";
import { usePhoneLayout } from "../../shell/phone/usePhoneLayout";
import { cx } from "../cx";
import { CaretDownIcon, CaretRightIcon } from "../icons";
import { ui } from "../recipes";
import { captioned, columnClass, keepToCell, PanelCards, PanelIntro } from "./PanelCard";

export interface PanelColumn<T> {
  header: ReactNode;
  /** Figures go right, names go left. Default left. */
  align?: "left" | "right";
  /** Classes for this column's cells; a function when the tone depends on the row. */
  className?: string | ((row: T) => string | undefined);
  cell: (row: T) => ReactNode;
  /** This column's figure in the totals row (with `totalLabel`). */
  total?: ReactNode;
  /** On a phone card (`phoneCard`): the value is a sentence, so it takes the card's whole width. */
  phoneWide?: boolean;
}

/** Attributes for one row: anything a `<tr>` takes, `data-*` included. */
export type PanelRowProps = HTMLAttributes<HTMLElement> & {
  [data: `data-${string}`]: string | number | boolean | undefined;
};

/** How a row reads as a card on a phone layout. Column indexes are into `columns`. */
export interface PanelPhoneCard<T> {
  /** The column that titles the card. Default 0. */
  title?: number;
  /** A column drawn at the title line's right end (a status), before the chevron. */
  aside?: number;
  /** One line under the title (a row's place and kind). */
  subtitle?: (row: T) => ReactNode;
  /** The columns drawn as label over value, in this order. Default every column but the title
   *  and the aside. An empty header draws the value alone. */
  figures?: readonly number[];
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
  /** Per-row attributes: on the desk row after the table's own (`className` joins
   *  `rowClassName`; `onClick` is the row's when the table does not open rows itself); on a phone
   *  card, its `data-*`, `aria-*`, `className` and `onClick`. */
  rowProps?: (row: T) => PanelRowProps;
  /** On a phone layout, draw each row as a card. Leave unset to keep the table on a phone. A card
   *  draws no totals row and does not open sub-rows. */
  phoneCard?: PanelPhoneCard<T>;
}

/** What every row of the desk table reads: the table's props that shape a row, and the one
 *  open expandable row. */
interface RowContext<T> {
  columns: readonly PanelColumn<T>[];
  rowKey: (row: T, index: number) => string;
  rowHeader: boolean;
  rowClassName?: (row: T) => string | undefined;
  onRowOpen?: (row: T) => void;
  isSelected?: (row: T) => boolean;
  actions?: (row: T) => ReactNode;
  subRows?: (row: T) => readonly T[] | null | undefined;
  rowProps?: (row: T) => PanelRowProps;
  open: string | null;
  setOpen: (key: string | null) => void;
}

/** A cell's classes: the name cell, a figure, a child row's first cell, the column's own. */
function cellClass(named: boolean, num: boolean, subFirst: boolean, own: string | undefined) {
  return cx(
    "oxy-ui-panel-table__cell",
    named && "oxy-ui-panel-table__cell--name",
    num && "oxy-ui-panel-table__cell--num",
    subFirst && "oxy-ui-panel-table__cell--sub",
    own
  );
}

/** A row's cells on the table's columns; the first holds the opener when there is one. */
function rowCells<T>(ctx: RowContext<T>, row: T, opener: ReactNode | null, sub: boolean) {
  return ctx.columns.map((c, j) => {
    const named = j === 0 && ctx.rowHeader;
    const Cell = named ? "th" : "td";
    return (
      <Cell
        key={j}
        scope={named ? "row" : undefined}
        className={cellClass(named, c.align === "right", sub && j === 0, columnClass(c, row))}
      >
        {j === 0 && opener ? opener : c.cell(row)}
      </Cell>
    );
  });
}

/** The keyboard's way in: the row's name as a button with no handler of its own, so its press
 *  bubbles to the row's, one press either way. */
function RowOpener({
  expandable,
  isOpen,
  selected,
  children
}: {
  expandable: boolean;
  isOpen: boolean;
  selected: boolean;
  children: ReactNode;
}) {
  return (
    <button
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
      {children}
    </button>
  );
}

/** One row's state: its key, its child rows, whether it opens, is open, is selected. */
function rowState<T>(ctx: RowContext<T>, row: T, i: number) {
  const key = ctx.rowKey(row, i);
  const kids = ctx.subRows?.(row) ?? [];
  const expandable = kids.length > 0;
  return {
    key,
    kids,
    expandable,
    isOpen: expandable && ctx.open === key,
    selected: ctx.isSelected?.(row) ?? false,
    interactive: expandable || ctx.onRowOpen !== undefined
  };
}

/** The desk row's classes: the row, an opening row, its selection or hover, the caller's. */
function rowClass<T>(
  ctx: RowContext<T>,
  row: T,
  state: ReturnType<typeof rowState<T>>,
  own: string | undefined
) {
  const { interactive, selected, isOpen } = state;
  return cx(
    "oxy-ui-panel-table__row",
    interactive && "oxy-ui-panel-table__row--open",
    interactive ? ui.rowState(selected || isOpen) : selected && ui.rowSelected,
    ctx.rowClassName?.(row),
    own
  );
}

/** A row, and its child rows under it while it is open. */
function rowLines<T>(ctx: RowContext<T>, row: T, i: number) {
  const state = rowState(ctx, row, i);
  const { key, kids, expandable, isOpen, interactive } = state;
  const press = () => {
    if (expandable) ctx.setOpen(isOpen ? null : key);
    else ctx.onRowOpen?.(row);
  };
  const opener = interactive ? (
    <RowOpener key='open' expandable={expandable} isOpen={isOpen} selected={state.selected}>
      {ctx.columns[0]?.cell(row)}
    </RowOpener>
  ) : null;
  const { className: ownClass, onClick: ownClick, ...attrs } = ctx.rowProps?.(row) ?? {};
  const line = (
    <tr
      key={key}
      {...attrs}
      className={rowClass(ctx, row, state, ownClass)}
      onClick={interactive ? press : ownClick}
    >
      {rowCells(ctx, row, opener, false)}
      {ctx.actions && (
        <td
          className='oxy-ui-panel-table__cell oxy-ui-panel-table__cell--actions'
          onClick={keepToCell}
        >
          {ctx.actions(row)}
        </td>
      )}
    </tr>
  );
  return isOpen ? [line, ...subLines(ctx, key, kids)] : [line];
}

/** An open row's children, on the parent's columns, the last closing the block. */
function subLines<T>(ctx: RowContext<T>, key: string, kids: readonly T[]) {
  return kids.map((kid, j) => (
    <tr
      key={`${key}/${ctx.rowKey(kid, j)}`}
      className='oxy-ui-panel-table__row oxy-ui-panel-table__sub-row'
      data-last={j === kids.length - 1 ? "" : undefined}
    >
      {rowCells(ctx, kid, null, true)}
      {ctx.actions && <td className='oxy-ui-panel-table__cell' />}
    </tr>
  ));
}

/** The column headings, and the actions column's hidden one. */
function HeadRow<T>({
  columns,
  actions,
  actionsLabel
}: {
  columns: readonly PanelColumn<T>[];
  actions: boolean;
  actionsLabel: string;
}) {
  return (
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
  );
}

/** The totals row: the label in the first cell, each column's `total` after it. */
function TotalsRow<T>({
  columns,
  totalLabel,
  actions
}: {
  columns: readonly PanelColumn<T>[];
  totalLabel: ReactNode;
  actions: boolean;
}) {
  return (
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
  );
}

/** The open expandable row: the caller's when it holds it (`expanded`), else the table's. */
function useOpenRow(
  expanded: string | null | undefined,
  onExpandedChange: ((key: string | null) => void) | undefined
) {
  const [held, setHeld] = useState<string | null>(null);
  const open = expanded !== undefined ? expanded : held;
  const setOpen = (key: string | null) => {
    if (expanded === undefined) setHeld(key);
    onExpandedChange?.(key);
  };
  return { open, setOpen };
}

export function PanelTable<T>(props: PanelTableProps<T>) {
  const {
    caption,
    label,
    columns,
    rows,
    rowKey,
    lead,
    footnote,
    rowHeader = true,
    actions,
    actionsLabel = "Actions",
    totalLabel,
    expanded,
    onExpandedChange,
    phoneCard,
    className,
    rowClassName: _rc,
    onRowOpen: _ro,
    isSelected: _is,
    subRows: _sr,
    rowProps: _rp,
    ...rest
  } = props;
  const phone = usePhoneLayout();
  const captionId = useId();
  const { open, setOpen } = useOpenRow(expanded, onExpandedChange);
  const labelledBy = captioned(caption) ? captionId : undefined;
  const ctx: RowContext<T> = { ...props, rowHeader, open, setOpen };
  return (
    <div {...rest} className={cx("oxy-ui-panel-table", className)}>
      <PanelIntro caption={caption} captionId={captionId} lead={lead} />
      {phone && phoneCard ? (
        <PanelCards {...props} form={phoneCard} labelledBy={labelledBy} label={label} />
      ) : (
        <table
          className={cx(
            "oxy-ui-panel-table__table",
            totalLabel !== undefined && "oxy-ui-panel-table__table--totals"
          )}
          aria-labelledby={labelledBy}
          aria-label={labelledBy ? undefined : label}
        >
          <thead>
            <HeadRow columns={columns} actions={Boolean(actions)} actionsLabel={actionsLabel} />
          </thead>
          <tbody>{rows.flatMap((row, i) => rowLines(ctx, row, i))}</tbody>
          {totalLabel !== undefined && (
            <tfoot>
              <TotalsRow columns={columns} totalLabel={totalLabel} actions={Boolean(actions)} />
            </tfoot>
          )}
        </table>
      )}
      {footnote && <p className={cx(ui.note, "oxy-ui-panel-table__note")}>{footnote}</p>}
    </div>
  );
}
