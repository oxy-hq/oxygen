import { useCallback, useEffect, useRef, useState } from "react";
import {
  canGoBack,
  canGoForward,
  currentEntry,
  EMPTY_HISTORY,
  isSameDocument,
  moveCursor,
  type PreviewHistoryState,
  pushEntry,
  replaceEntry,
  shouldInterceptAnchor
} from "./previewHistory";

/**
 * React binding for the preview's own history — see `previewHistory.ts` for
 * why the framed app must not write to the admin console's Back stack.
 *
 * The pure stack logic lives next door and is unit-tested there; this file is
 * only the part that needs a live frame: installing the guard on load, reading
 * the location back out, and driving `location.replace` when the operator uses
 * the preview's own controls.
 */

/**
 * What a `go()` actually did.
 *
 *  - `none` — there is no frame yet. The ordinary case for a deep link, since
 *    the admin URL is read on mount and the iframe has not fired `load`.
 *  - `same-document` — a fragment move, absorbed with `replaceState`. No load
 *    event will follow, so nothing is passing through and there is nothing for
 *    a deep-link handoff to suppress.
 *  - `cross-document` — a real navigation. A load event follows, and the
 *    document the frame is leaving will report itself on the way past.
 */
type PreviewNavigation = "none" | "same-document" | "cross-document";

export interface PreviewHistory {
  /** The location the preview is showing, absolute. `null` before first load. */
  url: string | null;
  canBack: boolean;
  canForward: boolean;
  back: () => void;
  forward: () => void;
  /** Re-navigate to the current entry without adding one. */
  reload: () => void;
  /**
   * Jump the preview somewhere — used to restore a URL off the admin's own
   * query string, so an admin link reproduces the previewed screen.
   *
   * Reports what it did, because both answers matter to the caller: `none`
   * means the destination must NOT be recorded (a caller that recorded it
   * anyway would believe the preview is somewhere it never went, and then skip
   * applying the link when the frame finally arrives), and `same-document`
   * means no load event is coming.
   */
  go: (url: string) => PreviewNavigation;
  /** False when the frame turned out not to be reachable (cross-origin, or
   *  torn down mid-call). The chrome hides itself rather than offering
   *  controls that silently do nothing. */
  available: boolean;
  /** Wire to the iframe's `onLoad`. Idempotent per document. */
  handleLoad: (frame: HTMLIFrameElement) => void;
  /** Call when the frame is being replaced (channel flip, reload nonce) so the
   *  stack starts clean rather than describing a document that is gone. */
  reset: () => void;
}

export function usePreviewHistory(): PreviewHistory {
  const [state, setState] = useState<PreviewHistoryState>(EMPTY_HISTORY);
  const [available, setAvailable] = useState(true);
  const frameRef = useRef<HTMLIFrameElement | null>(null);
  // The guard is installed per *document*. A frame that navigates for real
  // (a link we converted to `location.replace`) gets a fresh document and a
  // fresh `history` object, so the patch has to go back on at each load — and
  // must not stack, or `pushState` ends up wrapped N deep.
  const guardedDoc = useRef<Document | null>(null);
  const disposeGuard = useRef<(() => void) | null>(null);

  /** Navigate the live frame without growing the joint history. */
  const navigate = useCallback((url: string): PreviewNavigation => {
    const win = frameRef.current?.contentWindow;
    // No frame yet. Not an error — `LivePreview` reads the admin URL on mount
    // and the iframe has not loaded — but the caller has to know, or it records
    // a position the frame never took.
    if (!win) return "none";
    let kind: PreviewNavigation = "cross-document";
    try {
      if (isSameDocument(win.location.href, url)) {
        // Fragment-only: `location.replace` would reload the document for a
        // difference the document can handle itself.
        //
        // The PATCHED `replaceState` here, deliberately unlike the click path
        // above. This runs for a cursor move the stack has already made, or a
        // deep link `framePath` already records, so the report it triggers
        // rewrites an entry that holds the same URL and `replaceEntry` drops
        // it. Reaching for the original would mean the stack never hears about
        // a location the frame really is at, which is the failure that is hard
        // to see rather than the one that is.
        const from = win.location.href;
        kind = "same-document";
        win.history.replaceState(win.history.state, "", url);
        announceSameDocumentMove(win, from);
      } else {
        win.location.replace(url);
      }
    } catch {
      setAvailable(false);
      return "none";
    }
    return kind;
  }, []);

  /**
   * Re-fetch the current document.
   *
   * NOT `navigate(currentEntry)`. That target is by definition the location the
   * frame is already at, so `isSameDocument` is unconditionally true and the
   * call lands in the fragment branch — a `replaceState` to the same URL and a
   * synthetic `popstate`. No network, no reload: a dead control sitting beside
   * the toolbar's own working one, which is the confusing kind of dead.
   *
   * `location.reload()` on a same-origin frame replaces the current document
   * without adding a joint entry, which is the whole requirement.
   */
  const reload = useCallback(() => {
    const win = frameRef.current?.contentWindow;
    if (!win) return;
    try {
      win.location.reload();
    } catch {
      setAvailable(false);
    }
  }, []);

  // The cursor lives in state, but a click has to act on the value it just
  // computed — so the ref mirrors it and `step` reads from there. Deliberately
  // NOT a side effect inside a `setState` updater: an updater is re-invoked
  // under StrictMode and may be recomputed and discarded under concurrent
  // rendering, so navigating from inside one moves the frame for state that
  // never commits. Mirroring also makes two fast clicks on Back walk two
  // entries rather than recomputing from a stale render.
  const stateRef = useRef(state);
  const step = useCallback(
    (delta: number) => {
      const next = moveCursor(stateRef.current, delta);
      if (next === stateRef.current) return;
      stateRef.current = next;
      setState(next);
      const target = currentEntry(next);
      if (target) navigate(target);
    },
    [navigate]
  );
  // Anything that sets state elsewhere (a load, the app's own navigation) has
  // to reach the mirror too, or the next traversal starts from a stale cursor.
  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  const handleLoad = useCallback(
    (frame: HTMLIFrameElement) => {
      frameRef.current = frame;
      try {
        const win = frame.contentWindow;
        const doc = win?.document;
        if (!win || !doc) return;
        // A frame that has not navigated yet reports `about:blank`; recording
        // it would put an entry in the stack that Back could return to and that
        // renders nothing.
        if (win.location.href === "about:blank") return;

        if (guardedDoc.current !== doc) {
          disposeGuard.current?.();
          disposeGuard.current = installGuard(win, {
            onNavigate: (url, mode) =>
              setState((prev) =>
                mode === "push" ? pushEntry(prev, url) : replaceEntry(prev, url)
              ),
            onTraverse: step,
            onReload: reload
          });
          guardedDoc.current = doc;
        }

        setState((prev) => pushEntry(prev, win.location.href));
        setAvailable(true);
      } catch {
        // Cross-origin, or the frame was torn down between the load event and
        // this handler. Either way there is no history to offer.
        setAvailable(false);
      }
    },
    [step, reload]
  );

  // The hook installed the guard, so the hook drops it. Leaving that to the
  // consumer was the wrong owner — it left a listener bound to a closure over
  // an unmounted component, and in a test file (where several hooks share one
  // jsdom document) an abandoned guard intercepts the next one's clicks and
  // `preventDefault`s them out from under it.
  useEffect(
    () => () => {
      disposeGuard.current?.();
      disposeGuard.current = null;
    },
    []
  );

  const reset = useCallback(() => {
    disposeGuard.current?.();
    disposeGuard.current = null;
    guardedDoc.current = null;
    setState(EMPTY_HISTORY);
  }, []);

  return {
    url: currentEntry(state),
    canBack: canGoBack(state),
    canForward: canGoForward(state),
    back: () => step(-1),
    forward: () => step(1),
    reload,
    go: navigate,
    available,
    handleLoad,
    reset
  };
}

/**
 * Finish a same-document move that `replaceState` only half did.
 *
 * `replaceState` is deliberately silent — no `hashchange`, no `popstate`, and
 * no fragment scroll — so absorbing a hash navigation this way leaves three
 * jobs the browser would have done:
 *
 *  - **Scroll.** The default navigation was what scrolled to `#totals`, and we
 *    cancelled it. Without this an in-page anchor updates the URL and the page
 *    does not move, which reads as a dead link.
 *  - **`hashchange`**, for a router or listener watching the fragment.
 *  - **`popstate`**, because a `BrowserRouter`-style router listens for that and
 *    never for `hashchange` — without it React Router does not re-read the
 *    location, and the synthetic `hashchange` reaches nothing.
 *
 * Constructed in the **frame's** realm where possible, so an `instanceof` check
 * inside the app sees its own `Event`. TypeScript's `Window` does not declare
 * the constructors every real window carries, hence the cast; the fallback
 * covers a host that genuinely lacks it.
 */
function announceSameDocumentMove(win: Window, from?: string): void {
  const realm = win as unknown as {
    Event?: typeof Event;
    HashChangeEvent?: typeof HashChangeEvent;
    PopStateEvent?: typeof PopStateEvent;
  };
  const to = win.location.href;
  const hash = win.location.hash.slice(1);
  if (hash) {
    try {
      const target =
        win.document.getElementById(hash) ??
        win.document.querySelector(`[name="${CSS.escape(hash)}"]`);
      target?.scrollIntoView();
    } catch {
      /* an unparseable fragment is not worth failing the navigation over */
    }
  }

  // Typed events with their payload where the realm has the constructors: a
  // listener reading `e.newURL` / `e.oldURL` / `e.state` gets what the real
  // navigation would have given it. A bare `Event` is the degradation, not the
  // target — it was what the first version always sent.
  const HashChange = realm.HashChangeEvent;
  const PopState = realm.PopStateEvent;
  const Basic = realm.Event ?? Event;
  win.dispatchEvent(
    HashChange
      ? new HashChange("hashchange", { oldURL: from ?? to, newURL: to })
      : new Basic("hashchange")
  );
  win.dispatchEvent(
    PopState ? new PopState("popstate", { state: win.history.state }) : new Basic("popstate")
  );
}

/**
 * Stop `win` from ever appending to — or traversing — the joint session
 * history, and report where it goes instead.
 *
 * The three interception points and the reasoning for each are documented in
 * `previewHistory.ts`. Everything here is wrapped so a frame that becomes
 * cross-origin mid-call cannot take the admin console down with it.
 */
export interface GuardHooks {
  /** The frame moved to `url`. */
  onNavigate: (url: string, mode: "push" | "replace") => void;
  /** The frame asked to traverse its history by `delta` — routed into the
   *  preview's own stack instead of the joint one. */
  onTraverse: (delta: number) => void;
  /** The frame asked for `history.go(0)`, which the platform defines as a
   *  reload. Routed to a real one rather than to a zero-delta cursor move. */
  onReload: () => void;
}

export function installGuard(win: Window, hooks: GuardHooks): () => void {
  // The listeners are tied to a signal so they can be dropped. In a live frame
  // the document is discarded on navigation and they go with it, so this is
  // hygiene rather than a leak fix — but it is what lets the wiring be driven
  // more than once against one document, which is how it gets tested at all.
  const controller = new AbortController();
  const { signal } = controller;
  const history = win.history;
  const originalReplace = history.replaceState.bind(history);
  // Kept so the disposer can put the object back. A live document is discarded
  // on navigation so the patch would go with it — this is for the case the
  // disposer exists for at all: one document, guarded more than once.
  const originals = {
    pushState: history.pushState,
    replaceState: history.replaceState,
    back: history.back,
    forward: history.forward,
    go: history.go
  };

  // `pushState` becomes `replaceState`. The framed app cannot tell the
  // difference — both leave `location` where it asked — and the joint stack
  // stops growing. Our own stack takes the "push" instead, which is what the
  // preview's Back button then walks.
  history.pushState = function patchedPushState(data, unused, url) {
    originalReplace(data, unused, url);
    try {
      hooks.onNavigate(win.location.href, "push");
    } catch {
      /* reporting must never break the app's own navigation */
    }
  };

  history.replaceState = function patchedReplaceState(data, unused, url) {
    originalReplace(data, unused, url);
    try {
      hooks.onNavigate(win.location.href, "replace");
    } catch {
      /* as above */
    }
  };

  // An app with its own Back button calls `history.back()`. Now that the frame
  // contributes no joint entries, that traverses to the nearest entry there —
  // which is the ADMIN CONSOLE's, navigating the operator off the page
  // entirely. The old defect was "Back steps through the frame invisibly"; this
  // is the same surprise pointed the other way, and it arrives WITH the fix for
  // the first one. So traversal routes into the preview's own stack too.
  history.back = () => hooks.onTraverse(-1);
  history.forward = () => hooks.onTraverse(1);
  // `go()` and `go(0)` both mean reload in the platform, and routing them into
  // `moveCursor(s, 0)` makes them silent no-ops instead.
  history.go = (delta?: number) => (delta ? hooks.onTraverse(delta) : hooks.onReload());

  // A link the app does not intercept is a real navigation, and a real
  // navigation in an existing frame adds a joint entry no `history` patch can
  // prevent.
  //
  // BUBBLE phase, deliberately. React delegates at its root container, which is
  // a descendant of `document`, so by the time the event reaches here the app's
  // own handlers have run and `defaultPrevented` finally means what it says:
  // the app claimed this click. In capture phase the flag is always false — and
  // deferring the check to a microtask does NOT fix that, because calling
  // `preventDefault()` ourselves sets the flag, so the deferred check reads our
  // own cancellation, skips the replacement navigation, and leaves the link
  // dead. That was the shape this replaced.
  win.document.addEventListener(
    "click",
    (event) => {
      const mouse = event as MouseEvent;
      if (mouse.defaultPrevented || mouse.button !== 0) return;
      if (mouse.metaKey || mouse.ctrlKey || mouse.shiftKey || mouse.altKey) return;
      const target = mouse.target as Element | null;
      const anchor = target?.closest?.("a[href]") as HTMLAnchorElement | null;
      if (!anchor) return;

      const descriptor = {
        href: anchor.href,
        target: anchor.target,
        hasDownload: anchor.hasAttribute("download")
      };
      if (!shouldInterceptAnchor(descriptor, win.location.origin)) return;

      mouse.preventDefault();
      try {
        if (isSameDocument(win.location.href, anchor.href)) {
          // `originalReplace`, not the patched one, so this reports a **push**:
          // the browser pushes for a fragment navigation, and going through the
          // patch would report "replace" and overwrite the current entry —
          // leaving the preview's own Back unable to return to the previous
          // fragment. The joint history still gains nothing, which is the part
          // that has to stay true.
          //
          // ORDER IS LOAD-BEARING. `announceSameDocumentMove` dispatches a
          // `popstate`, and the guard's own `popstate` listener below reports a
          // *replace*. Announced first, that replace lands before the push and
          // overwrites the current entry with the new fragment — after which
          // the push is a no-op, because the stack is already on that URL. The
          // net effect is exactly the replace this branch exists to avoid, and
          // a mock-based assertion cannot see it: both calls happen, in the
          // wrong order. Push first and the popstate's replace rewrites an
          // entry that already holds the same URL, which `replaceEntry` drops.
          const from = win.location.href;
          originalReplace(win.history.state, "", anchor.href);
          hooks.onNavigate(win.location.href, "push");
          announceSameDocumentMove(win, from);
        } else {
          win.location.replace(anchor.href);
        }
      } catch {
        /* the frame went away — nothing to navigate */
      }
    },
    { signal }
  );

  // `pushState` is patched, but the app may have captured the original before
  // we got here (a module-scope `const push = history.pushState`). Nothing can
  // be done about that from outside — record it if it happens so the preview's
  // stack stays truthful even when the joint one also grew.
  win.addEventListener(
    "popstate",
    () => {
      try {
        hooks.onNavigate(win.location.href, "replace");
      } catch {
        /* ignore */
      }
    },
    { signal }
  );

  return () => {
    controller.abort();
    Object.assign(history, originals);
  };
}
