// Started from bmg-industries-oxy `apps/reorder-sheet/src/HoverHint.tsx` (INVENTORY section 2: the
// only tested copy, `HoverHint.test.ts`, and the only one that handles a phone's taps), merged with
// pokehouse-oxy Locations' and Warehouse's (`role="tooltip"`, Escape closes) and the reference's
// (`side`, the bubble portalled into an open <dialog>, `TapHintFrame`). Kept from BMG: `placeHint`
// and `pressIsOnControl` unchanged, hover only for a pointer that can hover, focus only from the
// keyboard, a used control closes its hint, a tap on words opens it until the next tap anywhere,
// a scroll or a resize, `wide` and `panel`, no bubble wider than the window. Kept from the
// reference: the look (the muted bubble on the control line), `side`. New: the focused control is
// described by the bubble (`aria-describedby`), Escape closes an open hint and only the hint (a
// panel around it stays), and `label` draws the ⓘ form Locations and Warehouse use: the children
// inside a real button with that name.
//
// 2.31.0 (the gap round): `focusable`, from pokehouse-oxy Bookkeeping's `components/HoverHint.tsx`
// (21 hints on words that are not a control: why a post failed, a drift figure, a warning mark),
// and the portal host is found only while a bubble shows, so the hint renders without a DOM
// (Inventory Management, Supply Chain and Warehouse render screens to markup in node tests).
//
// A hover tooltip that cannot be overflow-clipped: the bubble portals out with `position: fixed`
// and picks its direction from the anchor's place in the window. Inside an open <dialog> it
// portals into the dialog, which paints in the top layer: a bubble on <body> would sit behind it.
// It replaces the native `title`, which appears late, cannot wrap and vanishes before a two-line
// hint is read.
//
// **On a phone.** A tap is a hover, a focus and a click at once, and it has no mouseleave to undo
// the hover: a tap on a control opened its dialog AND its hint, and the hint stayed. So: hover
// opens a hint only for a pointer that can hover; focus opens one only from the keyboard; using
// the control a hint is on closes it; and a tap on a hint that is NOT a control (a number, a
// status, a header word) opens it until the next tap anywhere. A layout can also give a hint a
// tap form of its own (`TapHintFrame`, the phone kit's `PhoneHint`), which `OxyShell` does on a
// phone layout (2.31.1).

import {
  type FocusEvent,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState
} from "react";
import { createPortal } from "react-dom";
import { cx } from "../cx";

// The tap form of a hint, where the layout has one (the reference's): an ⓘ whose tap opens the
// words (the kit's `PhoneHint`). It lives in the shell since 2.31.1, so `OxyShell` can provide it on
// a phone layout; this entry exports the same context.
export { TapHintFrame, useTapHint } from "../../shell/phone/tapHintFrame";

/** The bubble's widest, by kind: a sentence, multi-line reference text, a grid. */
const MAX_W = 300;
const MAX_W_WIDE = 460;
const MAX_W_PANEL = 640;
/** The gap kept between a bubble and either edge of the window. */
const EDGE = 12;
/** How long a `panel` survives the pointer leaving its anchor: long enough to cross the 7px gap. */
const PANEL_GRACE_MS = 160;
/** A focus this soon after a touch on the anchor came from that touch, not the keyboard. */
const TOUCH_FOCUS_MS = 1000;

/** Where a bubble goes for an anchor at `r`, in a window `view` wide and high. */
export interface HintPlacement {
  top: number;
  /** The bubble's RIGHT edge: it is drawn right-aligned to this x. */
  left: number;
  below: boolean;
  /** Its widest, which is never more than the window less an edge each side. */
  maxWidth: number;
}

/**
 * Open into whichever half of the window has more room. The bubble is right-aligned to `left`, so
 * both sides are clamped: never past the right edge, and never so far left that a full-width
 * bubble runs off the left. On a window narrower than the bubble's usual width, it narrows to fit.
 */
export function placeHint(
  r: { top: number; bottom: number; right: number },
  view: { width: number; height: number },
  widest: number
): HintPlacement {
  const maxWidth = Math.max(0, Math.min(widest, view.width - 2 * EDGE));
  const below = r.top < view.height / 2;
  const rightEdge = Math.min(r.right + 6, view.width - EDGE);
  return {
    top: below ? r.bottom + 7 : r.top - 7,
    left: Math.max(rightEdge, maxWidth + EDGE),
    below,
    maxWidth
  };
}

/**
 * Whether a press is on a CONTROL, which a tap uses rather than explains: a hint marked
 * `on-control`, a column header (a tap there sorts), or anything inside a button, link, field or
 * label.
 */
export function pressIsOnControl(
  anchorClass: string | undefined,
  inHeader: boolean,
  target: unknown
): boolean {
  if ((anchorClass ?? "").split(/\s+/).includes("on-control") || inHeader) return true;
  const el = target as { closest?: (selector: string) => unknown } | null;
  return (
    typeof el?.closest === "function" &&
    el.closest("button, a[href], input, select, textarea, label, [role='button']") !== null
  );
}

export interface HoverHintProps {
  /** The hint, a sentence (line breaks kept). */
  text?: string;
  /** Structured content, for a hint that is a layout rather than a sentence; wins over `text`. */
  content?: ReactNode;
  /** On the anchor. `on-control` marks a hint on something a tap uses (it keeps its cursor). */
  className?: string;
  /** 460px rather than 300px, for multi-line reference text. */
  wide?: boolean;
  /** 640px, laid out as given, and reachable by the pointer (a grid with hints of its own). */
  panel?: boolean;
  /** Open to the RIGHT of the anchor, centred on it (an icon rail, where above lands on the
   *  item above). */
  side?: boolean;
  /** The ⓘ form: `children` (an icon) inside a real button with this name. */
  label?: string;
  /**
   * The words say something the screen does not otherwise say (why a post failed, what a mark
   * warns of), so the keyboard has to reach them: the anchor becomes a Tab stop that opens the
   * bubble on focus, and the words ride in its accessible text. Leave it off where the child is
   * already focusable, or the hint only restates what is visible: a Tab stop on every row's hint
   * would bury the row's own controls. The words are `text`'s; a `content` layout describes the
   * anchor instead.
   */
  focusable?: boolean;
  children: ReactNode;
}

type Placement = HintPlacement & { side?: boolean };

export function HoverHint({
  text,
  content,
  className,
  wide,
  panel,
  side,
  label,
  focusable = false,
  children
}: HoverHintProps) {
  const anchorRef = useRef<HTMLSpanElement>(null);
  const bubbleRef = useRef<HTMLDivElement>(null);
  const bubbleId = useId();
  const [pos, setPos] = useState<Placement | null>(null);
  // Opened by a tap: there is no mouseleave coming, so the next tap anywhere, a scroll or a
  // resize closes it instead.
  const [tapped, setTapped] = useState(false);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const press = useRef<{ type: string; at: number }>({ type: "", at: 0 });
  /** The control the bubble describes while it shows from focus. */
  const described = useRef<HTMLElement | null>(null);

  useEffect(
    () => () => {
      if (hideTimer.current !== null) clearTimeout(hideTimer.current);
      undescribe(described, bubbleId);
    },
    [bubbleId]
  );

  useEffect(() => {
    if (!tapped) return;
    const close = () => {
      setPos(null);
      setTapped(false);
    };
    // A tap on the anchor toggles it; a tap on the bubble closes it. Anything else closes it at
    // once, and still does what it does.
    const onDown = (e: Event) => {
      const t = e.target as Node | null;
      if (t && (anchorRef.current?.contains(t) || bubbleRef.current?.contains(t))) return;
      close();
    };
    document.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [tapped]);

  const words = content ?? text;
  // A focusable anchor says a sentence hint in its own text; a layout (`content`) describes it.
  const carriesWords = focusable && label === undefined && content == null && Boolean(text);
  // Nothing to say, nothing to wrap: an anchor with no bubble is a `cursor: help` that lies.
  if (words === undefined || words === null || words === "") return <>{children}</>;

  const cancelHide = () => {
    if (hideTimer.current === null) return;
    clearTimeout(hideTimer.current);
    hideTimer.current = null;
  };

  const show = () => {
    cancelHide();
    const r = anchorRef.current?.getBoundingClientRect();
    if (!r) return;
    if (side) {
      // Left edge just past the anchor, so the bubble grows away from the rail, not over it.
      setPos({ top: r.top + r.height / 2, left: r.right + 8, below: true, maxWidth: MAX_W, side });
      return;
    }
    const widest = panel ? MAX_W_PANEL : wide ? MAX_W_WIDE : MAX_W;
    setPos(placeHint(r, { width: window.innerWidth, height: window.innerHeight }, widest));
  };

  const hideNow = () => {
    cancelHide();
    undescribe(described, bubbleId);
    setPos(null);
    setTapped(false);
  };

  // A plain hint goes the instant the pointer leaves; a panel waits to be reached.
  const hide = () => {
    if (!panel) {
      hideNow();
      return;
    }
    cancelHide();
    hideTimer.current = setTimeout(hideNow, PANEL_GRACE_MS);
  };

  const touchedJustNow = () =>
    press.current.type === "touch" && Date.now() - press.current.at < TOUCH_FOCUS_MS;

  const onPointerEnter = (e: PointerEvent) => {
    if (e.pointerType !== "touch") show();
  };
  const onPointerLeave = (e: PointerEvent) => {
    if (e.pointerType !== "touch") hide();
  };
  const onPointerDown = (e: PointerEvent) => {
    press.current = { type: e.pointerType, at: Date.now() };
  };

  // Keyboard parity when the child is focusable. No tabIndex of its own: forcing every hinted cell
  // into the Tab order would be worse than a hint that is mouse-only on a plain span. A click or a
  // tap focuses a button too, and the hint is not what it asked for.
  const onFocus = (e: FocusEvent) => {
    if (touchedJustNow()) return;
    const el = e.target as HTMLElement;
    let keyboard = true;
    try {
      keyboard = typeof el.matches === "function" ? el.matches(":focus-visible") : true;
    } catch {
      // An engine without :focus-visible: any focus shows it.
    }
    if (!keyboard) return;
    // A focusable anchor carries the words itself: described by the bubble too, they would be
    // read twice.
    if (!(carriesWords && el === anchorRef.current)) describe(described, el, bubbleId);
    show();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    // Dismissible (WCAG 1.4.13): Escape closes the hint, and only the hint.
    if (e.key !== "Escape" || !pos) return;
    e.preventDefault();
    hideNow();
  };

  const onClick = (e: MouseEvent) => {
    // The control was used: its hint has done its job, and what the press opens must not open
    // under it.
    if (pressIsOnControl(className, anchorRef.current?.closest("thead") != null, e.target)) {
      hideNow();
      return;
    }
    // A tap on words to explain: the only way to read them on a phone.
    if (touchedJustNow()) {
      if (pos) hideNow();
      else {
        show();
        setTapped(true);
      }
    }
  };

  const onControl = (className ?? "").split(/\s+/).includes("on-control");
  // Found only while a bubble shows (after a hover, a focus or a tap), so rendering the hint
  // never reads the document: a screen renders to markup on a server, or in a test without one.
  const host = () => anchorRef.current?.closest("dialog") ?? document.body;

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: a wrapper that listens to its children (hover, focus, a tap); it is no control itself.
    <span
      ref={anchorRef}
      className={cx(
        "oxy-ui-hover-hint",
        (onControl || label !== undefined) && "oxy-ui-hover-hint--on-control",
        focusable && cx("oxy-ui-hover-hint--focusable", "oxy-ui-focus"),
        className
      )}
      tabIndex={focusable && label === undefined ? 0 : undefined}
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
      onPointerDown={onPointerDown}
      onFocus={onFocus}
      onBlur={hide}
      onKeyDown={onKeyDown}
      onClick={label === undefined ? onClick : undefined}
    >
      {label === undefined ? (
        <>
          {children}
          {carriesWords && <span className='oxy-ui-hover-hint__words'>{` ${text}`}</span>}
        </>
      ) : (
        <button type='button' aria-label={label} className='oxy-ui-hover-hint__mark oxy-ui-focus'>
          {children}
        </button>
      )}
      {pos &&
        createPortal(
          // biome-ignore lint/a11y/useKeyWithClickEvents: a tap on a tapped bubble closes it; Escape on the anchor is the keyboard's way.
          <div
            ref={bubbleRef}
            id={bubbleId}
            role='tooltip'
            className={cx(
              "oxy-ui-hover-hint__bubble",
              pos.side
                ? "oxy-ui-hover-hint__bubble--side"
                : pos.below
                  ? "oxy-ui-hover-hint__bubble--below"
                  : "oxy-ui-hover-hint__bubble--above",
              wide && "oxy-ui-hover-hint__bubble--wide",
              panel && "oxy-ui-hover-hint__bubble--panel",
              tapped && "oxy-ui-hover-hint__bubble--tapped"
            )}
            style={{ top: pos.top, left: pos.left, maxWidth: pos.maxWidth }}
            onMouseEnter={panel ? cancelHide : undefined}
            onMouseLeave={panel ? hide : undefined}
            onClick={tapped ? hideNow : undefined}
          >
            {words}
          </div>,
          host()
        )}
    </span>
  );
}

/** Adds the bubble's id to `el`'s description, remembering `el` to take it off again. */
function describe(slot: { current: HTMLElement | null }, el: HTMLElement, id: string): void {
  undescribe(slot, id);
  const ids = (el.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
  if (!ids.includes(id)) el.setAttribute("aria-describedby", [...ids, id].join(" "));
  slot.current = el;
}

function undescribe(slot: { current: HTMLElement | null }, id: string): void {
  const el = slot.current;
  slot.current = null;
  if (!el) return;
  const ids = (el.getAttribute("aria-describedby") ?? "").split(/\s+/).filter((x) => x && x !== id);
  if (ids.length) el.setAttribute("aria-describedby", ids.join(" "));
  else el.removeAttribute("aria-describedby");
}
