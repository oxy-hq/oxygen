// The universal top bar: breadcrumb (left) + status, clock, and host-provided
// actions (right). Ported from web-app/src/components/Shell/TopBar — this is
// now the source of truth; the web-app composes it with its own Ask Oxygen
// toggle in the `right` slot.

import { type ReactNode, useEffect, useMemo, useState } from "react";
import { cx } from "./cx";
import { ShellPortalContext, useShellPortalContainer } from "./portal";

/** Frame only: left content + right cluster. Mirrors the rail's
 *  `--sidebar-background` so the two read as one 48px-tall frame. */
export function TopBar({
  left,
  right,
  className
}: {
  left?: ReactNode;
  right?: ReactNode;
  className?: string;
}) {
  const { container, setContainer } = useShellPortalContainer();
  return (
    <ShellPortalContext.Provider value={container}>
      <header
        ref={setContainer}
        data-testid='workspace-topbar'
        className={cx("oxy-shell-scope oxy-topbar", className)}
      >
        {left}
        <div className='oxy-topbar__right'>{right}</div>
      </header>
    </ShellPortalContext.Provider>
  );
}

/**
 * "<Workspace> / <Page>" — e.g. "Poke House / HQ". The workspace name links
 * back to the HQ home; pass `onHomeNavigate` to intercept for SPA routing
 * (the anchor still carries `homeHref` for middle-click/new-tab).
 */
export function Breadcrumb({
  workspaceLabel,
  pageLabel,
  homeHref,
  onHomeNavigate
}: {
  workspaceLabel: string;
  pageLabel: string;
  homeHref: string;
  onHomeNavigate?: () => void;
}) {
  return (
    <div className='oxy-breadcrumb'>
      <a
        href={homeHref}
        data-testid='topbar-workspace-link'
        className='oxy-breadcrumb__workspace'
        onClick={
          onHomeNavigate
            ? (e) => {
                // Let modified clicks (new tab, download, …) fall through to
                // the browser; plain left-clicks stay in the SPA.
                if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
                e.preventDefault();
                onHomeNavigate();
              }
            : undefined
        }
      >
        {workspaceLabel}
      </a>
      <span className='oxy-breadcrumb__sep'>/</span>
      <span className='oxy-breadcrumb__page'>{pageLabel}</span>
    </div>
  );
}

/** `navigator.onLine` + online/offline events. */
function useOnlineStatus(): boolean {
  const [online, setOnline] = useState(() =>
    typeof navigator === "undefined" ? true : navigator.onLine
  );
  useEffect(() => {
    const up = () => setOnline(true);
    const down = () => setOnline(false);
    window.addEventListener("online", up);
    window.addEventListener("offline", down);
    return () => {
      window.removeEventListener("online", up);
      window.removeEventListener("offline", down);
    };
  }, []);
  return online;
}

/**
 * Universal "Sys: Connected" indicator — a pulsing green dot when the browser
 * is online, red when offline. Presentational connectivity signal; a real
 * backend health ping can replace `useOnlineStatus` later without touching
 * consumers.
 */
export function SystemIndicator() {
  const online = useOnlineStatus();
  return (
    <span data-testid='topbar-system' className='oxy-shell-scope oxy-sysind'>
      <span>Sys:</span>
      <span
        className={cx(
          "oxy-sysind__state",
          online ? "oxy-sysind__state--online" : "oxy-sysind__state--offline"
        )}
      >
        <span className='oxy-sysind__dot-wrap'>
          {online && <span className='oxy-sysind__ping' />}
          <span
            className={cx(
              "oxy-sysind__dot",
              online ? "oxy-sysind__dot--online" : "oxy-sysind__dot--offline"
            )}
          />
        </span>
        {online ? "Connected" : "Offline"}
      </span>
    </span>
  );
}

/**
 * The clock's text in the given IANA timezone (viewer-local when omitted or
 * invalid). Updates each half-minute (minute precision), pausing while the
 * tab is hidden. Shared by `WorkspaceClock` and the wired shell's Today clock.
 */
export function useClockText(timezone?: string): string {
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    const update = () => setNow(new Date());
    const id = setInterval(() => {
      if (!document.hidden) update();
    }, 30_000);
    // Re-show: tick immediately so the clock isn't up to 30s stale after the
    // tab was backgrounded (interval ticks are skipped while hidden).
    const onVisible = () => {
      if (!document.hidden) update();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);

  return useMemo(() => {
    try {
      return new Intl.DateTimeFormat(undefined, {
        timeZone: timezone,
        hour: "numeric",
        minute: "2-digit",
        timeZoneName: timezone ? "short" : undefined
      }).format(now);
    } catch {
      // Invalid IANA name → local time without a zone label rather than crash.
      return new Intl.DateTimeFormat(undefined, {
        hour: "numeric",
        minute: "2-digit"
      }).format(now);
    }
  }, [now, timezone]);
}

/** Live clock in the given IANA timezone (viewer-local when omitted or invalid). */
export function WorkspaceClock({ timezone }: { timezone?: string }) {
  const text = useClockText(timezone);
  return (
    <span data-testid='topbar-clock' className='oxy-shell-scope oxy-clock'>
      {text}
    </span>
  );
}
