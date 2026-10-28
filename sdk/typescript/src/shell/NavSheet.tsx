// The rail's narrow-screen form: a menu button (in the top bar) opening a
// left drawer of the same entries, each with its name. On a touch screen the
// rail's only labels are hover tooltips, which never show, so below 768px
// OxyShell hides the rail and renders this instead (see `.oxy-shell--navsheet`
// in shell.css). Mirrors web-app/src/components/Shell/ShellNavSheet.tsx;
// written without a dialog library because the SDK carries none.

import { Fragment, type ReactNode, type RefObject, useEffect, useRef, useState } from "react";
import { cx } from "./cx";
import { type RailItem, RailItemMark } from "./ShellRail";

function MenuIcon() {
  return (
    <svg
      width='20'
      height='20'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      aria-hidden='true'
    >
      <path d='M4 6h16M4 12h16M4 18h16' />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg
      width='16'
      height='16'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      aria-hidden='true'
    >
      <path d='M18 6 6 18M6 6l12 12' />
    </svg>
  );
}

function NavRow({ item, onPick }: { item: RailItem; onPick: () => void }) {
  const className = cx("oxy-navsheet__row", item.active && "oxy-navsheet__row--active");
  const ariaCurrent = item.active ? ("page" as const) : undefined;
  const testId = item.testId ? `nav-sheet-${item.testId}` : undefined;
  const content = (
    <>
      {/* Decorative: the label names the row, and a letter mark would be read
          into it ("BBookings"). */}
      <span aria-hidden='true' className='oxy-navsheet__icon'>
        <RailItemMark item={item} />
      </span>
      <span className='oxy-navsheet__label'>{item.label}</span>
    </>
  );
  // Every pick closes the drawer; an anchor still follows its href.
  return item.href ? (
    <a
      href={item.href}
      data-testid={testId}
      aria-current={ariaCurrent}
      className={className}
      onClick={onPick}
    >
      {content}
    </a>
  ) : (
    <button
      type='button'
      data-testid={testId}
      aria-current={ariaCurrent}
      className={className}
      onClick={() => {
        onPick();
        item.onSelect?.();
      }}
    >
      {content}
    </button>
  );
}

/** Open state shared by the menu button and the drawer, which OxyShell
 *  renders apart: the button in the top bar, the drawer last in the shell. */
export type NavSheetState = {
  open: boolean;
  setOpen: (open: boolean) => void;
  triggerRef: RefObject<HTMLButtonElement | null>;
};

export function useNavSheet(): NavSheetState {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  return { open, setOpen, triggerRef };
}

type NavSheetContentProps = {
  title: string;
  top?: ReactNode;
  groups: RailItem[][];
  footerItems?: RailItem[];
  bottom?: ReactNode;
};

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

// Tab and Shift+Tab wrap inside the panel: it is `aria-modal`, so focus must
// not walk out to the page hidden behind the overlay.
function trapTab(e: KeyboardEvent, panel: HTMLElement | null) {
  if (!panel) return;
  const focusable = [...panel.querySelectorAll<HTMLElement>(FOCUSABLE)];
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  if (!first || !last) return;
  const active = document.activeElement;
  if (!panel.contains(active)) {
    e.preventDefault();
    first.focus();
  } else if (e.shiftKey && active === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && active === last) {
    e.preventDefault();
    first.focus();
  }
}

/** The menu button. Hidden by shell.css except below 768px in an
 *  `.oxy-shell--navsheet` shell. */
export function NavSheetTrigger({ sheet }: { sheet: NavSheetState }) {
  return (
    <button
      ref={sheet.triggerRef}
      type='button'
      data-testid='nav-sheet-trigger'
      aria-label='Open navigation'
      aria-expanded={sheet.open}
      className='oxy-navsheet-trigger'
      onClick={() => sheet.setOpen(true)}
    >
      <MenuIcon />
    </button>
  );
}

/** The drawer. Render it after the page content: at the same z-index a
 *  later element paints on top, so an app's own `fixed` bar stays under it. */
export function NavSheetPanel({
  sheet,
  title,
  top,
  groups,
  footerItems,
  bottom
}: NavSheetContentProps & { sheet: NavSheetState }) {
  const { open, setOpen, triggerRef } = sheet;
  const panelRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const close = () => setOpen(false);

  // Focus moves into the drawer on open and back to the menu button on close,
  // so a keyboard user is never stranded behind the overlay. Escape closes,
  // and so does widening past the breakpoint, where the rail is back and an
  // open drawer would only cover it.
  useEffect(() => {
    if (!open) return;
    closeRef.current?.focus();
    const trigger = triggerRef.current;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
      else if (e.key === "Tab") trapTab(e, panelRef.current);
    };
    const wide = window.matchMedia?.("(min-width: 768px)");
    const onWide = () => {
      if (wide?.matches) setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    wide?.addEventListener?.("change", onWide);
    return () => {
      window.removeEventListener("keydown", onKey);
      wide?.removeEventListener?.("change", onWide);
      trigger?.focus();
    };
  }, [open, setOpen, triggerRef]);

  if (!open) return null;
  return (
    <div className='oxy-navsheet'>
      <div className='oxy-navsheet__overlay' aria-hidden='true' onClick={close} />
      <div
        ref={panelRef}
        role='dialog'
        aria-modal='true'
        aria-label={title || "Navigation"}
        data-testid='nav-sheet'
        className='oxy-navsheet__panel'
      >
        <div className='oxy-navsheet__header'>
          {top}
          <span className='oxy-navsheet__title'>{title}</span>
          <button
            ref={closeRef}
            type='button'
            aria-label='Close navigation'
            className='oxy-navsheet__close'
            onClick={close}
          >
            <CloseIcon />
          </button>
        </div>
        <nav className='oxy-navsheet__nav'>
          {groups.map((group, i) => (
            <Fragment key={group[0]?.key ?? i}>
              {i > 0 && <div className='oxy-navsheet__divider' />}
              {group.map((item) => (
                <NavRow key={item.key} item={item} onPick={close} />
              ))}
            </Fragment>
          ))}
          {footerItems && footerItems.length > 0 && (
            <div className='oxy-navsheet__footer'>
              <div className='oxy-navsheet__divider' />
              {footerItems.map((item) => (
                <NavRow key={item.key} item={item} onPick={close} />
              ))}
            </div>
          )}
        </nav>
        {bottom && <div className='oxy-navsheet__bottom'>{bottom}</div>}
      </div>
    </div>
  );
}

/** Menu button + left drawer listing `ShellRail`'s entries with their names,
 *  side by side. OxyShell renders the two parts apart (see `NavSheetPanel`). */
export function NavSheet(props: NavSheetContentProps) {
  const sheet = useNavSheet();
  return (
    <>
      <NavSheetTrigger sheet={sheet} />
      <NavSheetPanel sheet={sheet} {...props} />
    </>
  );
}
