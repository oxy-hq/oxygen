// PanelTable's phone card form (2.32.0, `phoneCard`; decision 99), and the parts the table and
// the cards share: the caption and lead over them, a column's classes for a row, and the press
// that stays in an actions cell. The table decides which form to draw (PanelTable.tsx); this file
// draws the cards. The card is Quadrogen's own (`PhoneRows`), in tokens: see tables.css.

import type { HTMLAttributes, MouseEvent, ReactNode } from "react";
import { cx } from "../cx";
import { CaretRightIcon } from "../icons";
import { ui } from "../recipes";
import type { PanelColumn, PanelPhoneCard, PanelRowProps } from "./PanelTable";

/** A press inside the actions cell is the action's, not the row's. */
export const keepToCell = (event: MouseEvent) => event.stopPropagation();

/** A column's own classes for one row. */
export function columnClass<T>(c: PanelColumn<T>, row: T): string | undefined {
  return typeof c.className === "function" ? c.className(row) : c.className;
}

const present = (node: ReactNode) =>
  node !== undefined && node !== null && node !== false && node !== "";

/** The table has a caption to be named by (an empty string still counts, as it always has). */
export const captioned = (caption: ReactNode) =>
  caption !== undefined && caption !== null && caption !== false;

/** The eyebrow caption (the table's name) and the lead sentence, over either form. */
export function PanelIntro({
  caption,
  captionId,
  lead
}: {
  caption: ReactNode;
  captionId: string;
  lead: ReactNode;
}) {
  return (
    <>
      {captioned(caption) && (
        <h3 id={captionId} className={cx(ui.eyebrow, "oxy-ui-panel-table__caption")}>
          {caption}
        </h3>
      )}
      {lead && <p className={cx(ui.note, "oxy-ui-panel-table__note")}>{lead}</p>}
    </>
  );
}

export interface PanelCardsProps<T> {
  columns: readonly PanelColumn<T>[];
  rows: readonly T[];
  rowKey: (row: T, index: number) => string;
  form: PanelPhoneCard<T>;
  /** The caption's id when the list is named by it; else `label` names it. */
  labelledBy: string | undefined;
  label: string | undefined;
  rowProps?: (row: T) => PanelRowProps;
  rowClassName?: (row: T) => string | undefined;
  isSelected?: (row: T) => boolean;
  onRowOpen?: (row: T) => void;
  actions?: (row: T) => ReactNode;
}

/** The rows as a list of cards. */
export function PanelCards<T>({
  columns,
  rows,
  rowKey,
  form,
  labelledBy,
  label,
  rowProps,
  rowClassName,
  isSelected,
  onRowOpen,
  actions
}: PanelCardsProps<T>) {
  return (
    <ul
      className='oxy-ui-panel-table__cards'
      aria-labelledby={labelledBy}
      aria-label={labelledBy ? undefined : label}
    >
      {rows.map((row, i) => (
        <PanelCard
          key={rowKey(row, i)}
          row={row}
          columns={columns}
          form={form}
          attrs={rowProps?.(row)}
          className={rowClassName?.(row)}
          selected={isSelected?.(row) ?? false}
          onOpen={onRowOpen && (() => onRowOpen(row))}
          actions={actions}
        />
      ))}
    </ul>
  );
}

interface PanelCardProps<T> {
  row: T;
  columns: readonly PanelColumn<T>[];
  form: PanelPhoneCard<T>;
  attrs: PanelRowProps | undefined;
  className: string | undefined;
  selected: boolean;
  onOpen: (() => void) | undefined;
  actions: ((row: T) => ReactNode) | undefined;
}

/** What of a row's attributes a card takes: `data-*`, `aria-*`, the class and the click. A hover
 *  title has no hover on a phone, and the card's own button takes the keyboard. */
function cardAttrs(attrs: PanelRowProps | undefined) {
  const {
    className,
    onClick,
    onKeyDown: _key,
    tabIndex: _tab,
    title: _hover,
    ...data
  } = attrs ?? {};
  return { className, onClick, data: data as HTMLAttributes<HTMLLIElement> };
}

/** The figure columns: the form's, or every column but the title and the aside. */
function figureColumns<T>(columns: readonly PanelColumn<T>[], form: PanelPhoneCard<T>) {
  const t = form.title ?? 0;
  return form.figures ?? columns.map((_, j) => j).filter((j) => j !== t && j !== form.aside);
}

/** One row as a card: the title line (title, then the aside and the chevron at its end), the
 *  subtitle, the figures. A row that opens is pressed anywhere on the card; its title is the
 *  button the keyboard reaches, with no handler of its own, so its press bubbles to the card's. */
function PanelCard<T>({
  row,
  columns,
  form,
  attrs,
  className,
  selected,
  onOpen,
  actions
}: PanelCardProps<T>) {
  const own = cardAttrs(attrs);
  const open = onOpen ?? own.onClick;
  const subtitle = form.subtitle?.(row);
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the press anywhere is the pointer's; the keyboard's is the title's button, whose press bubbles here.
    <li
      {...own.data}
      className={cx(
        ui.tile,
        ui.paddingSm,
        "oxy-ui-panel-table__card",
        open && "oxy-ui-panel-table__card--open",
        className,
        own.className
      )}
      onClick={open as HTMLAttributes<HTMLLIElement>["onClick"]}
    >
      <CardHead row={row} columns={columns} form={form} opens={Boolean(open)} selected={selected} />
      {present(subtitle) && <div className='oxy-ui-panel-table__card-subtitle'>{subtitle}</div>}
      <CardFigures row={row} columns={columns} figures={figureColumns(columns, form)} />
      {actions && (
        // biome-ignore lint/a11y/useKeyWithClickEvents: it only stops a press reaching the card; the actions are controls of their own.
        // biome-ignore lint/a11y/noStaticElementInteractions: see above
        <div className='oxy-ui-panel-table__card-actions' onClick={keepToCell}>
          {actions(row)}
        </div>
      )}
    </li>
  );
}

/** The title line: the title (the card's button when it opens), then the aside and the chevron. */
function CardHead<T>({
  row,
  columns,
  form,
  opens,
  selected
}: {
  row: T;
  columns: readonly PanelColumn<T>[];
  form: PanelPhoneCard<T>;
  opens: boolean;
  selected: boolean;
}) {
  const head = columns[form.title ?? 0];
  const titleClass = head && columnClass(head, row);
  const title = head?.cell(row);
  const hasAside = form.aside !== undefined;
  return (
    <div className='oxy-ui-panel-table__card-head'>
      {opens ? (
        <button
          type='button'
          className={cx(
            "oxy-ui-panel-table__card-title",
            "oxy-ui-panel-table__card-open",
            ui.focusRing,
            titleClass
          )}
          aria-current={selected ? "true" : undefined}
        >
          {title}
        </button>
      ) : (
        <span className={cx("oxy-ui-panel-table__card-title", titleClass)}>{title}</span>
      )}
      {(hasAside || opens) && (
        <span className='oxy-ui-panel-table__card-trailing'>
          {hasAside && columns[form.aside as number]?.cell(row)}
          {opens && (
            <CaretRightIcon className={cx(ui.iconControl, "oxy-ui-panel-table__card-chevron")} />
          )}
        </span>
      )}
    </div>
  );
}

/** The figures, label over value; a `phoneWide` one across the card, an empty header left out. */
function CardFigures<T>({
  row,
  columns,
  figures
}: {
  row: T;
  columns: readonly PanelColumn<T>[];
  figures: readonly number[];
}) {
  if (figures.length === 0) return null;
  return (
    <dl className='oxy-ui-panel-table__card-figures'>
      {figures.map((j) => {
        const c = columns[j];
        if (!c) return null;
        return (
          <div
            key={j}
            className={cx(
              "oxy-ui-panel-table__card-figure",
              c.phoneWide && "oxy-ui-panel-table__card-figure--wide"
            )}
          >
            {present(c.header) && (
              <dt className={cx(ui.text2xs, "oxy-ui-panel-table__card-label")}>{c.header}</dt>
            )}
            <dd className={cx("oxy-ui-panel-table__card-value", columnClass(c, row))}>
              {c.cell(row)}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}
