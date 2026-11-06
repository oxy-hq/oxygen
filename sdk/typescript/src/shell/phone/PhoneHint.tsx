// The ⓘ that explains a number, in place of a hover-only tooltip a touch
// screen never shows. A tap pins it open and a second tap, Escape, a tap
// elsewhere or a scroll closes it. A mouse opens it on hover and a keyboard on
// focus, as a tooltip would. The pointer type is checked because a phone fires
// emulated mouse and focus events on a tap, and a hint that opened on those
// would close again on the click that follows.
//
// The explanation is a small popover portaled into the shell scope (so its
// tokens and a `.dark` ancestor still apply), placed under the ⓘ or above it
// when there is no room, and always kept 16px inside the viewport. An explanation
// taller than the space (a phone on its side) scrolls inside the popover, and
// that scroll does not close it.

import {
  type FocusEvent,
  type PointerEvent,
  type ReactNode,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState
} from "react";
import { createPortal } from "react-dom";
import { ShellPortalContext } from "../portal";
import { InfoIcon } from "./icons";

export interface PhoneHintProps {
  /** The button's accessible name: "About same-store sales". */
  label: string;
  /** The explanation. */
  children: ReactNode;
}

type Mode = "closed" | "peek" | "pinned";

const EDGE = 16;
const GAP = 8;
const WIDTH = 280;

function place(button: HTMLElement, pop: HTMLElement) {
  const r = button.getBoundingClientRect();
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const width = Math.min(WIDTH, vw - EDGE * 2);
  const left = Math.min(Math.max(r.left + r.width / 2 - width / 2, EDGE), vw - EDGE - width);
  pop.style.width = `${width}px`;
  pop.style.left = `${left}px`;
  pop.style.maxHeight = "";
  const height = Math.min(pop.offsetHeight, vh - EDGE * 2);
  pop.style.maxHeight = `${vh - EDGE * 2}px`;
  const below = r.bottom + GAP;
  const above = r.top - GAP - height;
  if (below + height <= vh - EDGE) pop.style.top = `${below}px`;
  else if (above >= EDGE) pop.style.top = `${above}px`;
  // Fits on neither side of the ⓘ: all of it stays on screen, over the ⓘ if need be.
  else pop.style.top = `${Math.max(EDGE, vh - EDGE - height)}px`;
}

function focusVisible(el: Element): boolean {
  try {
    return el.matches(":focus-visible");
  } catch {
    return false; // an engine without the selector: treat focus as a click's
  }
}

export function PhoneHint({ label, children }: PhoneHintProps) {
  const container = useContext(ShellPortalContext);
  const [mode, setMode] = useState<Mode>("closed");
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  // Set while Escape hands focus back: that focus must not reopen the hint.
  const closing = useRef(false);
  const id = useId();
  const open = mode !== "closed";

  useLayoutEffect(() => {
    if (open && buttonRef.current && popRef.current) place(buttonRef.current, popRef.current);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const close = () => setMode("closed");
    const onScroll = (e: Event) => {
      if (!(e.target instanceof Node && popRef.current?.contains(e.target))) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Capture phase, stopped: Escape closes the hint, not the sheet under it.
      e.stopPropagation();
      close();
      // Back to the ⓘ only from nowhere or from inside the hint: someone typing
      // in a field while the mouse rests on the ⓘ keeps their place.
      const active = document.activeElement;
      if (!active || active === document.body || popRef.current?.contains(active)) {
        closing.current = true;
        buttonRef.current?.focus();
        closing.current = false;
      }
    };
    const onPointerDown = (e: globalThis.PointerEvent) => {
      const target = e.target as Node;
      if (buttonRef.current?.contains(target) || popRef.current?.contains(target)) return;
      close();
    };
    window.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      document.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
    };
  }, [open]);

  const onPointerEnter = (e: PointerEvent<HTMLButtonElement>) => {
    if (e.pointerType === "mouse") setMode((m) => (m === "closed" ? "peek" : m));
  };
  const onPointerLeave = (e: PointerEvent<HTMLButtonElement>) => {
    if (e.pointerType === "mouse") setMode((m) => (m === "peek" ? "closed" : m));
  };
  const onFocus = (e: FocusEvent<HTMLButtonElement>) => {
    if (!closing.current && focusVisible(e.currentTarget))
      setMode((m) => (m === "closed" ? "peek" : m));
  };
  // Focus moving to another control closes it either way; a tap on nothing
  // focusable (relatedTarget null) closes only a peek — the outside-pointerdown
  // listener handles a pinned one.
  const onBlur = (e: FocusEvent<HTMLButtonElement>) => {
    const next = e.relatedTarget as Node | null;
    if (next && !popRef.current?.contains(next)) setMode("closed");
    else setMode((m) => (m === "peek" ? "closed" : m));
  };

  return (
    <>
      <button
        ref={buttonRef}
        type='button'
        aria-label={label}
        aria-expanded={open}
        aria-describedby={open ? id : undefined}
        className='oxy-shell-scope oxy-phone-hint'
        // A click pins a peeking hint (so a hover-then-click keeps it) and
        // closes a pinned one.
        onClick={() => setMode((m) => (m === "pinned" ? "closed" : "pinned"))}
        onPointerEnter={onPointerEnter}
        onPointerLeave={onPointerLeave}
        onFocus={onFocus}
        onBlur={onBlur}
      >
        <InfoIcon />
      </button>
      {open &&
        createPortal(
          <div
            ref={popRef}
            id={id}
            role='tooltip'
            data-testid='phone-hint'
            className='oxy-shell-scope oxy-phone oxy-phone-hint__pop'
          >
            {children}
          </div>,
          container ?? document.body
        )}
    </>
  );
}
