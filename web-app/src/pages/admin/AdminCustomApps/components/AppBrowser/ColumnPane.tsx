import { type MouseEvent, type ReactNode, useEffect, useRef } from "react";
import { cn } from "@/libs/shadcn/utils";
import { type SelectHow, useColumnKeys } from "./useColumnKeys";

export interface ColumnRow {
  id: string;
  /** What type-ahead matches, and what a screen reader reads. */
  label: string;
  /** Where this row leads, so ⌘-click still opens it in a tab. */
  href: string;
  children: ReactNode;
}

/**
 * One tier of the browser: a label, a scrolling list, one selection.
 *
 * **Rows are anchors carrying `role="option"`.** Both halves are load-bearing and they
 * pull against each other. The anchor is for the mouse — an operator ⌘-clicks an app to
 * open it beside the one they are in, and copies a row's link into a ticket, which is why
 * `FleetRow` is a link too. The option role is for the keyboard and for assistive tech,
 * because what this is, behaviourally, is a listbox: one selection, arrow keys, type-ahead.
 * A plain click is intercepted and handled as a selection; a modified or middle click is
 * left to the browser, so the href is not decoration.
 *
 * **The selection is drawn twice over.** A column that does not have focus shows its
 * selection faintly (`bg-accent`) and the focused column shows its own strongly
 * (`bg-muted`) — Finder's behaviour, and the thing that tells you which of three
 * highlighted rows your arrow keys are about to move. Both are theme tokens; no colour
 * is introduced, because on this surface colour means something is wrong.
 */
export const ColumnPane = ({
  id,
  label,
  depth,
  width,
  rows,
  selectedId,
  empty,
  onSelect,
  onLeaveColumn
}: {
  /** Testid suffix and accessible name stem: `orgs`, `apps`, `pages`. */
  id: string;
  label: string;
  /** Which tier this is, so the rail can move focus between columns by number. */
  depth: number;
  /**
   * A Tailwind width class — the tiers are deliberately different widths, and the two
   * that hold identities grow at `2xl`.
   *
   * **Fixed, not fluid, and that is the Finder part.** A column whose width tracks the
   * viewport makes every row reflow when the window moves, and the whole value of this
   * shape is that a row stays where your eye left it. But fixed at one width wastes a
   * 27-inch screen: measured at 2560px, the Apps tier was still truncating
   * `Authz Differe…` with about 900px of the pane standing empty. So the tiers step up
   * once, at `2xl`, rather than tracking anything.
   */
  width: string;
  rows: readonly ColumnRow[];
  selectedId: string | null;
  /** One line for a tier that legitimately has nothing in it. */
  empty: string;
  onSelect: (id: string, how: SelectHow) => void;
  onLeaveColumn: (delta: -1 | 1) => void;
}) => {
  const index = rows.findIndex((r) => r.id === selectedId);
  const onKeyDown = useColumnKeys({
    labels: rows.map((r) => r.label),
    index,
    onPick: (next) => onSelect(rows[next].id, "key"),
    onLeaveColumn
  });

  // **Focus follows the selection, but only inside the column that already has it.**
  // A roving tabindex means exactly one row is focusable, so after an arrow key moves the
  // selection the old row keeps the focus ring while the new one holds the only tab stop:
  // the keyboard still works (the handler is on the listbox) but the ring points at the
  // wrong row and a screen reader announces nothing. The containment check is what keeps
  // this from stealing focus when the selection changed somewhere else — a click in
  // another column, the path bar, or a pasted URL.
  // The row is found by `selectedId` rather than by `[tabindex="0"]` so that the id is
  // what the effect actually reads — the same thing it re-runs on.
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const list = listRef.current;
    if (!list?.contains(document.activeElement)) return;
    const wanted =
      selectedId === null ? "[role='option']" : `[data-row-id="${CSS.escape(selectedId)}"]`;
    const option = list.querySelector<HTMLElement>(wanted);
    if (option && option !== document.activeElement) option.focus();
  }, [selectedId]);

  const pick = (event: MouseEvent<HTMLAnchorElement>, rowId: string) => {
    // Let the browser have the clicks that mean "somewhere else": ⌘/ctrl for a new tab,
    // shift for a new window, anything but the primary button.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    onSelect(rowId, "pointer");
  };

  return (
    <div
      data-browser-depth={depth}
      data-testid={`admin-apps-browser-column-${id}`}
      className={cn("flex min-h-0 shrink-0 flex-col border-r", width)}
    >
      <div className='shrink-0 border-b px-2 py-1.5 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
        {label}
      </div>
      {rows.length === 0 ? (
        <p className='px-2 py-2 text-muted-foreground text-xs'>{empty}</p>
      ) : (
        // A `div` rather than a `ul`: a `ul`'s children have to be `li`s, and an `li`
        // between the listbox and its options breaks the ownership a listbox needs.
        <div
          ref={listRef}
          role='listbox'
          aria-label={label}
          onKeyDown={onKeyDown}
          className='group/col min-h-0 flex-1 overflow-y-auto p-1'
        >
          {rows.map((row, i) => (
            <a
              key={row.id}
              href={row.href}
              role='option'
              aria-selected={row.id === selectedId}
              // Exactly one tab stop per column: the selection, or the first row when
              // there is none, so Tab reaches a column that has not been visited.
              tabIndex={(index < 0 ? i === 0 : row.id === selectedId) ? 0 : -1}
              onClick={(event) => pick(event, row.id)}
              data-row-id={row.id}
              // The real id stays on `data-row-id`; the testid is its selector-safe
              // spelling. An app's id is `org/slug`, and an unquoted CSS attribute
              // selector cannot hold a `/` — so `[data-testid=…-apps-local-oxy-starter]`
              // works the way `admin-apps-fleet-row-local-oxy-starter` already does,
              // instead of forcing every caller to quote and escape.
              data-testid={`admin-apps-browser-row-${id}-${row.id.replaceAll("/", "-")}`}
              className={cn(
                "flex h-8 w-full items-center gap-2 px-2 text-muted-foreground text-xs transition-colors",
                "hover:bg-accent hover:text-foreground",
                "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                "aria-selected:bg-accent aria-selected:text-foreground",
                "group-focus-within/col:aria-selected:bg-muted group-focus-within/col:aria-selected:font-medium"
              )}
            >
              {row.children}
            </a>
          ))}
        </div>
      )}
    </div>
  );
};
