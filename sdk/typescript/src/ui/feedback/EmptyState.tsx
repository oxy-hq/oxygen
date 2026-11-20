// Started from pokehouse-oxy `apps/store-ops/src/chrome/ui.tsx:142` `EmptyState` (BEYOND-GALLERY
// row 5: the only named copy with actions and an icon, on the `note` recipe; the reference has no
// empty state at all). Kept: `title`, `children`, `actions`, `icon`, `className` and the centred
// column. New: the no-match form (`query`, `onClear`): a search that found nothing says what was
// searched, offers to clear it, is announced as it appears, and puts focus back in the search
// field once cleared, since Clear leaves with the empty state.
//
// Says the read worked and found nothing, and what makes something appear: "No reports yet. The
// first one you submit shows up here." Not a failure (that is `LoadError`).

import type { ReactNode, RefObject } from "react";
import { Button } from "../actions/Button";
import { cx } from "../cx";
import { ui } from "../recipes";

export interface EmptyStateProps {
  /** The claim: "No reports yet". With `query`, defaults to "Nothing matches “<query>”". */
  title?: ReactNode;
  /** What makes something appear, or the next step. */
  children?: ReactNode;
  /** Buttons for that next step. */
  actions?: ReactNode;
  /** A glyph in a small tile above the title, decorative. */
  icon?: ReactNode;
  /** A search that found nothing: what was searched for. */
  query?: string;
  /** Clears the search (shows "Clear search" with `query`). */
  onClear?: () => void;
  /** The clear button's word. Default "Clear search". */
  clearLabel?: string;
  /** Where focus goes once the search is cleared: the search field. */
  searchRef?: RefObject<HTMLElement | null>;
  className?: string;
}

export function EmptyState({
  title,
  children,
  actions,
  icon,
  query,
  onClear,
  clearLabel = "Clear search",
  searchRef,
  className
}: EmptyStateProps) {
  const noMatch = query !== undefined;
  const claim = title ?? (noMatch ? `Nothing matches “${query}”` : null);
  return (
    <div
      className={cx("oxy-ui-empty-state", className)}
      // A search that empties the list is said as it happens, while the person types.
      role={noMatch ? "status" : undefined}
    >
      {icon && (
        <span className='oxy-ui-empty-state__icon' aria-hidden='true'>
          {icon}
        </span>
      )}
      {claim && <p className='oxy-ui-empty-state__title'>{claim}</p>}
      {children && <p className={cx(ui.note, "oxy-ui-empty-state__body")}>{children}</p>}
      {(actions || (noMatch && onClear)) && (
        <div className='oxy-ui-empty-state__actions'>
          {noMatch && onClear && (
            <Button
              onClick={() => {
                onClear();
                searchRef?.current?.focus();
              }}
            >
              {clearLabel}
            </Button>
          )}
          {actions}
        </div>
      )}
    </div>
  );
}
