// Started from oxy-hq/reference-app@81ee8aa `src/components/ReorderSheet.tsx` (INVENTORY section 4:
// 0 copies outside the reference, rebuilt there from BMG's `apps/reorder-sheet`). Kept, top to
// bottom in the same reading order: a toolbar with find on the left and the actions on the right,
// the one primary action counting the selection ("Draft order · 3"); a `ToggleChipGroup` that folds
// column groups to a stub; a `DataSheet` with identity pinned left, the status stripe on its first
// cell and the selection pinned right; a line when the find matches nothing. New, as a layout
// with slots (decision 55's assumption): the columns, the groups, the row's status, what a row's
// text is for the find, which rows can be picked and every word are the caller's; the sheet owns
// the frame and three pieces of state (the find, the folded groups, the selection), each of which
// the caller may hold instead (`value` + `onChange`). The primary action is `aria-disabled`, not
// `disabled`, while nothing is picked, so a commit that clears the selection keeps focus on it.
// The no-match line is `EmptyState`'s, with Clear search, focus back in the field; and `footer`, a
// slot under the sheet.
//
// What it leaves out, on purpose (the reference's own note): an attention line above the sheet is
// `AlertStrip`, stacked by the screen; how to slice rows into views is the screen's decision.
//
// No phone form of its own: DataSheet keeps its first column and scrolls the rest at phone width,
// and the toolbar wraps; a screen that wants cards on a phone switches to the phone kit itself.

import { type ReactNode, useRef } from "react";
import { Button } from "../actions";
import { cx } from "../cx";
import { EmptyState } from "../feedback";
import { matchesQuery, SearchField, ToggleChip, ToggleChipGroup } from "../forms";
import { ui } from "../recipes";
import { DataSheet, type PinnedColumn, type SheetGroup } from "../tables";
import type { StatusTone } from "../tone";
import { toggled, useControlled } from "./state";

export interface ReorderSheetSearch<R> {
  /** The field's name and placeholder: "Search SKU or item". */
  label: string;
  /** What of a row the find reads: one string or several (`[sku, name]`). */
  text: (row: R) => string | readonly (string | null | undefined)[];
}

export interface ReorderSheetCommit {
  /** The primary action's label for a count: (n) => `Draft order · ${n}`. */
  label: (count: number) => ReactNode;
  /** Takes the picked row keys. Without it the button counts and goes nowhere. */
  onCommit?: (keys: string[]) => void;
}

export interface ReorderSheetProps<R> {
  /** The sheet's accessible name, and its scroll box's while it scrolls. */
  label: string;
  rows: readonly R[];
  rowKey: (row: R) => string;
  /** Identity, pinned left at fixed widths (DataSheet). */
  pinned: PinnedColumn<R>[];
  /** Column groups; each gets a toggle in the row above the sheet. */
  groups: SheetGroup<R>[];
  /** The row's status, as a stripe down its first cell. */
  rowTone?: (row: R) => StatusTone | undefined;
  /** The find field. Omit for none. */
  search?: ReorderSheetSearch<R>;
  /** The find's text, held by the caller. */
  query?: string;
  onQueryChange?: (query: string) => void;
  /** The caps label over the toggles, and their group's name. Default "Columns". */
  togglesLabel?: string;
  /** Folded group ids, held by the caller; else the sheet's own, starting at `defaultHidden`. */
  hidden?: ReadonlySet<string>;
  defaultHidden?: ReadonlySet<string>;
  onHiddenChange?: (hidden: ReadonlySet<string>) => void;
  /** Which rows can be picked; the rest show a dash in the selection column. Omit for no
   *  selection column. */
  selectable?: (row: R) => boolean;
  /** The selection column's heading. Default "Select". */
  selectHeader?: string;
  /** A row's check box name. Default "Select <row key>". */
  selectLabel?: (row: R) => string;
  /** Picked row keys, held by the caller; else the sheet's own, starting empty. */
  selected?: ReadonlySet<string>;
  onSelectedChange?: (selected: ReadonlySet<string>) => void;
  /** Toolbar actions before the primary one: an Export CSV button. */
  actions?: ReactNode;
  /** The one primary action, fed by the selection. */
  commit?: ReorderSheetCommit;
  /** Under the sheet: a footnote or footer actions; a function gets the picked keys. */
  footer?: ReactNode | ((selected: string[]) => ReactNode);
  /** The no-match title for a query. Default EmptyState's "Nothing matches “…”". */
  noMatch?: (query: string) => ReactNode;
  /** The sheet's height before it scrolls. Default DataSheet's 30rem. */
  maxHeight?: string;
  className?: string;
}

const NONE: ReadonlySet<string> = new Set();

export function ReorderSheet<R>({
  label,
  rows,
  rowKey,
  pinned,
  groups,
  rowTone,
  search,
  query: heldQuery,
  onQueryChange,
  togglesLabel = "Columns",
  hidden: heldHidden,
  defaultHidden = NONE,
  onHiddenChange,
  selectable,
  selectHeader = "Select",
  selectLabel,
  selected: heldSelected,
  onSelectedChange,
  actions,
  commit,
  footer,
  noMatch,
  maxHeight,
  className
}: ReorderSheetProps<R>) {
  const [query, setQuery] = useControlled(heldQuery, "", onQueryChange);
  const [hidden, setHidden] = useControlled(heldHidden, defaultHidden, onHiddenChange);
  const [selected, setSelected] = useControlled(heldSelected, NONE, onSelectedChange);
  const field = useRef<HTMLInputElement>(null);

  const shown =
    search && query.trim() !== "" ? rows.filter((r) => matchesQuery(search.text(r), query)) : rows;
  const picked = [...selected];
  const nothingPicked = picked.length === 0;

  return (
    <div className={cx("oxy-ui-reorder-sheet", className)}>
      {(search || actions || commit) && (
        <div className='oxy-ui-reorder-sheet__toolbar'>
          {search && (
            <SearchField
              ref={field}
              label={search.label}
              value={query}
              onChange={setQuery}
              className='oxy-ui-reorder-sheet__search'
            />
          )}
          {(actions || commit) && (
            <div className='oxy-ui-reorder-sheet__actions'>
              {actions}
              {commit && (
                <Button
                  variant='primary'
                  // Not `disabled`: a commit that clears the selection would drop focus to <body>.
                  aria-disabled={nothingPicked || undefined}
                  onClick={() => {
                    if (!nothingPicked) commit.onCommit?.(picked);
                  }}
                >
                  {commit.label(picked.length)}
                </Button>
              )}
            </div>
          )}
        </div>
      )}

      {groups.length > 0 && (
        <ToggleChipGroup label={togglesLabel}>
          {groups.map((g) => (
            <ToggleChip
              key={g.id}
              pressed={!hidden.has(g.id)}
              onPressedChange={(on) => setHidden(toggled(hidden, g.id, !on))}
            >
              {g.label}
            </ToggleChip>
          ))}
        </ToggleChipGroup>
      )}

      {shown.length === 0 && query.trim() !== "" ? (
        <EmptyState
          query={query.trim()}
          title={noMatch?.(query.trim())}
          onClear={() => setQuery("")}
          searchRef={field}
          className='oxy-ui-reorder-sheet__empty'
        />
      ) : (
        <DataSheet
          label={label}
          rows={shown}
          rowKey={rowKey}
          pinned={pinned}
          groups={groups}
          hidden={hidden}
          rowTone={rowTone}
          maxHeight={maxHeight}
          action={
            selectable
              ? {
                  label: selectHeader,
                  render: (r) => {
                    const key = rowKey(r);
                    if (!selectable(r))
                      return (
                        <span className='oxy-ui-reorder-sheet__dash' aria-hidden='true'>
                          —
                        </span>
                      );
                    return (
                      <label className='oxy-ui-reorder-sheet__pick'>
                        <input
                          type='checkbox'
                          className={cx("oxy-ui-reorder-sheet__check", ui.focusRing)}
                          aria-label={selectLabel ? selectLabel(r) : `Select ${key}`}
                          checked={selected.has(key)}
                          onChange={(e) => setSelected(toggled(selected, key, e.target.checked))}
                        />
                      </label>
                    );
                  }
                }
              : undefined
          }
        />
      )}

      {footer != null && (
        <div className='oxy-ui-reorder-sheet__footer'>
          {typeof footer === "function" ? footer(picked) : footer}
        </div>
      )}
    </div>
  );
}
