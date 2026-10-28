// `<NotificationsBell />` — a bell with an unread badge that opens the inbox.
//
// Drop it in the app's header. It renders nothing until the inbox answers, and
// nothing at all where there is none (an older server, nobody signed in), so
// it is safe in every app.
//
// Styling follows `<OxyAnswer>`: inline styles only, no stylesheet and no
// framework. Colors are CSS variables, each falling back to the shell's
// (`--oxy-shell-*`, so it matches `<OxyShell>`), then a Tailwind v4 theme
// token (`--color-*`), then a neutral light default. Set any of:
//
//   --oxy-notifications-bg        panel background
//   --oxy-notifications-fg        text
//   --oxy-notifications-muted     secondary text
//   --oxy-notifications-border    borders and dividers
//   --oxy-notifications-hover     row and button hover
//   --oxy-notifications-accent    unread dot
//   --oxy-notifications-badge     badge background
//   --oxy-notifications-critical  the edge of a `critical` notification
//   --oxy-notifications-time-sensitive  the edge of a `time_sensitive` one
//   --oxy-notifications-z         panel z-index (default 50)

import * as React from "react";
import {
  appBasePath,
  badgeLabel,
  joinAppPath,
  type NotificationGroup,
  type NotificationItem,
  type NotificationsScope,
  notificationTarget,
  pageNavigation
} from "./notifications";
import { useNotifications } from "./notifications-hooks";
import { BellPanel } from "./notifications-panel";
import { useResolvedManifest } from "./react";

export interface NotificationsBellProps {
  /** `"app"` (default): this app's notifications. `"all"`: the whole inbox. */
  scope?: NotificationsScope;
  /**
   * Open one of this app's notifications in the app's own router, given the
   * path it passed to `ctx.notify` (`"/bookings/1"`). Without it the page
   * navigates to that path under the app's base path.
   */
  onNavigate?: (link: string, item: NotificationItem) => void;
  /**
   * The Oxygen origin, for a standalone dev server whose origin is not
   * Oxygen's. Other apps' notifications and Oxygen's own open there, in a new
   * tab. Leave unset in production.
   */
  productBaseUrl?: string;
  /** Which edge of the bell the panel lines up with. Default `"end"`. */
  align?: "start" | "end";
  /** How often to poll the unread count, in ms. Default 30 000. */
  pollInterval?: number;
  /** Accessible name. Default "Notifications". */
  label?: string;
  /** Class on the wrapper. */
  className?: string;
  /** Class on the panel. */
  panelClassName?: string;
}

const TOKEN = {
  fg: "var(--oxy-notifications-fg, var(--oxy-shell-foreground, var(--color-foreground, #0a0a0a)))",
  hover: "var(--oxy-notifications-hover, var(--oxy-shell-accent, var(--color-muted, #f4f4f5)))",
  badge:
    "var(--oxy-notifications-badge, var(--oxy-shell-destructive, var(--color-status-error, #dc2626)))"
};

/**
 * The person's inbox, inside the app. Must be rendered inside
 * `<OxyAppProvider>`.
 *
 * ```tsx
 * <header>
 *   <h1>Bookings</h1>
 *   <NotificationsBell onNavigate={(link) => navigate(link)} />
 * </header>
 * ```
 */
export function NotificationsBell(props: NotificationsBellProps): React.JSX.Element | null {
  const { scope = "app", align = "end", label = "Notifications" } = props;
  const inbox = useNotifications({ scope, pollInterval: props.pollInterval });
  const [open, setOpen] = React.useState(false);
  const [hover, setHover] = React.useState(false);
  const buttonRef = React.useRef<HTMLButtonElement>(null);
  const follow = useFollow(props);

  const close = React.useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  }, []);
  const toggle = () => {
    if (!open) void inbox.refresh();
    setOpen(!open);
  };
  const onItem = ({ item, more }: NotificationGroup) => {
    // Opening a thread reads all of it. Read-only on a staging or dev host:
    // the tap still opens it.
    if (!inbox.readOnly) {
      for (const one of [item, ...more]) if (one.read_at === null) void inbox.markRead(one.id);
    }
    if (follow(item)) close(false);
  };

  if (!inbox.available) return null;
  const badge = badgeLabel(inbox.unread);
  return (
    <div
      className={props.className}
      style={{ position: "relative", display: "inline-flex" }}
      data-testid='oxy-notifications'
    >
      <button
        ref={buttonRef}
        type='button'
        onClick={toggle}
        aria-label={badge ? `${label}, ${inbox.unread} unread` : label}
        aria-haspopup='dialog'
        aria-expanded={open}
        data-testid='oxy-notifications-bell'
        style={{ ...styles.button, background: hover || open ? TOKEN.hover : "transparent" }}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
      >
        <BellIcon />
        {badge ? (
          <span style={styles.badge} data-testid='oxy-notifications-badge' aria-hidden='true'>
            {badge}
          </span>
        ) : null}
      </button>
      {open ? (
        <BellPanel
          inbox={inbox}
          anchor={buttonRef}
          align={align}
          label={label}
          showApp={scope === "all"}
          className={props.panelClassName}
          onItem={onItem}
          onClose={close}
        />
      ) : null}
    </div>
  );
}

/**
 * Open an item; true when the person was taken somewhere. This app's open in
 * place, by `onNavigate` or under the app's base path; another app or an Oxygen
 * page in this tab, as the dock opens them; a page outside Oxygen in a new tab.
 */
function useFollow(props: NotificationsBellProps): (item: NotificationItem) => boolean {
  const { appId, orgSlug, appSlug, manifest } = useResolvedManifest();
  const { onNavigate, productBaseUrl } = props;
  return React.useCallback(
    (item: NotificationItem) => {
      const target = notificationTarget(item, { appId, productBaseUrl });
      if (target.kind === "none") return false;
      if (target.kind === "away") {
        // `_self` is this platform's own page, opened in this tab.
        if (target.window) pageNavigation.open(target.url, target.window);
        else pageNavigation.open(target.url, "_blank", "noopener,noreferrer");
        return true;
      }
      if (onNavigate) {
        onNavigate(target.link, item);
        return true;
      }
      const base = appBasePath(
        window.location.pathname,
        orgSlug || manifest.orgSlug || "",
        appSlug || manifest.slug
      );
      pageNavigation.assign(joinAppPath(base, target.link));
      return true;
    },
    [appId, orgSlug, appSlug, manifest, onNavigate, productBaseUrl]
  );
}

/** Lucide "bell", inlined. */
function BellIcon(): React.JSX.Element {
  return (
    <svg
      width='18'
      height='18'
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='2'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
    >
      <path d='M10.268 21a2 2 0 0 0 3.464 0' />
      <path d='M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326' />
    </svg>
  );
}

const styles: Record<string, React.CSSProperties> = {
  button: {
    position: "relative",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    width: 32,
    height: 32,
    padding: 0,
    border: "none",
    borderRadius: 6,
    color: TOKEN.fg,
    cursor: "pointer"
  },
  badge: {
    position: "absolute",
    top: 1,
    right: 0,
    minWidth: 16,
    height: 16,
    padding: "0 4px",
    boxSizing: "border-box",
    borderRadius: 8,
    background: TOKEN.badge,
    color: "#ffffff",
    fontFamily:
      '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    fontSize: 10,
    fontWeight: 600,
    lineHeight: "16px",
    textAlign: "center",
    pointerEvents: "none"
  }
};
