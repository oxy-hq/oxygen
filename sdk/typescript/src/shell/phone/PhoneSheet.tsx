// A full-screen sheet for a phone view: a store's detail, the filters behind a
// PhoneFilterChip, the tab bar's More list. It replaces the side panel or
// popover a desktop layout opens, which on a phone either covers half the
// screen or opens off it. Written without a dialog library because the SDK
// carries none (see NavSheet).

import { type ReactNode, useContext, useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import { cx } from "../cx";
import { trapTab } from "../NavSheet";
import { ShellPortalContext } from "../portal";
import { ChevronLeftIcon } from "./icons";
import { focusOrigin, isTopLayer, lockScroll, pushLayer } from "./layers";
import { holdHistoryEntry } from "./sheetHistory";
import { present } from "./tone";

export interface PhoneSheetProps {
  open: boolean;
  /** Called by Back, by Escape and by the phone's Back. The sheet does not close itself. */
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  /** Pinned under the body, e.g. one 48px `PhoneButton`. */
  footer?: ReactNode;
  backLabel?: string;
  /** Hold a history entry while open, so the phone's Back closes this sheet and
   *  leaves the page where it was. Pass `false` for a sheet that opened by pushing
   *  its own entry (a route, `?item=`): that entry already does the job, and
   *  Back's move through it is what closes the sheet. */
  historyEntry?: boolean;
}

export function PhoneSheet(props: PhoneSheetProps) {
  // Mount the panel only while open, so its effects (focus, lock, Escape) run
  // exactly for the time the sheet is on screen.
  return props.open ? <PhoneSheetPanel {...props} /> : null;
}

function PhoneSheetPanel({
  onClose,
  title,
  children,
  footer,
  backLabel = "Back",
  historyEntry = true
}: PhoneSheetProps) {
  const container = useContext(ShellPortalContext);
  const panelRef = useRef<HTMLDivElement>(null);
  const backRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  // The latest onClose, so an inline arrow from the app does not re-run the
  // open effect (and steal focus back) on every render.
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    const layer = Symbol("phone-sheet");
    const removeLayer = pushLayer(layer);
    const unlock = lockScroll();
    const opener = focusOrigin();
    const onKey = (e: KeyboardEvent) => {
      if (!isTopLayer(layer)) return;
      if (e.key === "Escape") closeRef.current();
      else if (e.key === "Tab") trapTab(e, panelRef.current);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      removeLayer();
      unlock();
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  useEffect(() => {
    if (historyEntry) return holdHistoryEntry(() => closeRef.current());
  }, [historyEntry]);

  // Apart from the effect above: the shell's portal container arrives after its
  // first commit, and a sheet open by then is rebuilt there with a new Back.
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-runs for a new container.
  useEffect(() => {
    backRef.current?.focus();
  }, [container]);

  return createPortal(
    <div
      ref={panelRef}
      role='dialog'
      aria-modal='true'
      aria-labelledby={titleId}
      data-testid='phone-sheet'
      className={cx(
        "oxy-shell-scope oxy-phone oxy-phone-sheet",
        present(footer) && "oxy-phone-sheet--footer"
      )}
    >
      <div className='oxy-phone-sheet__header'>
        <button
          ref={backRef}
          type='button'
          className='oxy-phone-sheet__back'
          onClick={() => closeRef.current()}
        >
          <ChevronLeftIcon />
          <span>{backLabel}</span>
        </button>
        <h2 id={titleId} className='oxy-phone-sheet__title'>
          {title}
        </h2>
      </div>
      <div className='oxy-phone-sheet__body'>{children}</div>
      {present(footer) && <div className='oxy-phone-sheet__footer'>{footer}</div>}
    </div>,
    container ?? document.body
  );
}
