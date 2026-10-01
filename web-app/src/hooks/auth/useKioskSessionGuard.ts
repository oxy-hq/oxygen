import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { clearLocalSession, useAuth } from "@/contexts/AuthContext";
import { redirectToCentralLogin } from "@/libs/orgSubdomain";
import { storedTokenSubject } from "@/libs/utils/authStorage";
import { mayBeKioskBrowser } from "@/libs/utils/kioskBrowser";
import ROUTES from "@/libs/utils/routes";
import type { KioskDevice } from "@/types/frontline";
import { kioskDeviceQueryOptions } from "./useFrontline";

/** How often an open, signed-in page on a kiosk asks again. */
export const KIOSK_SESSION_CHECK_MS = 60_000;

/**
 * `checking` — hold the page: this kiosk has not answered since the page
 * mounted. `ended` — the stored sign-in is being torn down and the browser
 * sent to sign-in. `ok` — render: not a kiosk, not signed in, still backed by
 * the cookie, or the answer is unknown.
 */
export type KioskSessionState = "checking" | "ok" | "ended";

/**
 * On an enrolled kiosk, the web app's stored sign-in must not outlive the
 * session cookie.
 *
 * The web app keeps a bearer token in `localStorage`, and nothing server-side
 * revokes it. A sign-out that happens outside the web app — a custom app's
 * idle timer calling `GET /api/logout`, which clears only the HttpOnly cookie —
 * therefore left an admin signed in to Oxygen on a shared tablet. So on a
 * kiosk, a signed-in page asks the kiosk probe whose session the cookie
 * carries: on mount, when the tab becomes visible or focused, and every
 * {@link KIOSK_SESSION_CHECK_MS}. When the cookie carries none, or someone
 * else's, the stored sign-in is cleared exactly as `logout` clears it and the
 * browser goes to sign-in, which on a kiosk is the crew's name board.
 *
 * Silent everywhere else: a browser with neither kiosk signal — the flag this
 * origin remembered, or the server's `oxy_kiosk_hint` cookie, which reaches an
 * org subdomain the flag never does (see `kioskBrowser`) — or holding no token
 * (a crew member's PIN session never stores one), makes no call. An unknown
 * answer — probe failed or answered 503, server too old to say — never signs
 * anyone out, and no answer this check gets ever switches it off.
 */
export function useKioskSessionGuard(): KioskSessionState {
  const { authConfig, isLocalMode } = useAuth();
  const tokenSubject = storedTokenSubject();
  const active = authConfig.auth_enabled && !isLocalMode && hasStoredToken() && mayBeKioskBrowser();

  const {
    data: device,
    isFetchedAfterMount,
    refetch
  } = useQuery({
    ...kioskDeviceQueryOptions,
    enabled: active,
    // A cached answer can predate the sign-out this exists to catch.
    refetchOnMount: "always",
    // The listeners below own focus and visibility, both of them.
    refetchOnWindowFocus: false,
    refetchInterval: active ? KIOSK_SESSION_CHECK_MS : false
  });

  useEffect(() => {
    if (!active) return;
    const recheck = () => {
      if (document.visibilityState === "visible") {
        // Join a check already in flight rather than start a second one:
        // switching back to the tab fires both events.
        void refetch({ cancelRefetch: false });
      }
    };
    document.addEventListener("visibilitychange", recheck);
    window.addEventListener("focus", recheck);
    return () => {
      document.removeEventListener("visibilitychange", recheck);
      window.removeEventListener("focus", recheck);
    };
  }, [active, refetch]);

  const ended = active && isFetchedAfterMount && kioskSessionEnded(device, tokenSubject);

  useEffect(() => {
    if (ended) {
      endKioskSession();
    }
  }, [ended]);

  if (!active) return "ok";
  if (ended) return "ended";
  return isFetchedAfterMount ? "ok" : "checking";
}

/**
 * True when a kiosk's session cookie no longer backs the stored token: the
 * cookie carries no session, or another user's. False for anything unknown —
 * not a kiosk (which is also how a failed probe reads), or a server that does
 * not report the cookie.
 */
function kioskSessionEnded(device: KioskDevice | undefined, tokenSubject: string | null): boolean {
  if (!device?.bound || device.sessionUserId === undefined) {
    return false;
  }
  return device.sessionUserId === null || device.sessionUserId !== tokenSubject;
}

function endKioskSession(): void {
  clearLocalSession();
  // A hard navigation, like the 401 handler's, so in-memory state from the
  // signed-in page does not survive into the sign-in page.
  if (!redirectToCentralLogin()) {
    window.location.href = ROUTES.AUTH.LOGIN;
  }
}

function hasStoredToken(): boolean {
  try {
    return !!localStorage.getItem("auth_token");
  } catch {
    return false;
  }
}
