/**
 * Whether this browser may be an enrolled store kiosk — a hint about whether
 * to ask, never the answer: `GET /frontline/device` decides.
 *
 * The kiosk cookie is HttpOnly, so the page cannot read it. Two signals stand
 * in for it, and the kiosk session check (`useKioskSessionGuard`) asks the
 * server only when one of them is present, so a browser that was never a kiosk
 * makes no extra call:
 *
 * - **The remembered flag**, in this origin's `localStorage`. Set when a probe
 *   answers "bound"; cleared only by an act that knows the kiosk is gone —
 *   leaving kiosk mode, or revoking this browser's own kiosk. A probe that
 *   answers "not bound" never clears it: one misread answer must not switch
 *   the check off for good.
 * - **The hint cookie** `oxy_kiosk_hint`, which the server sets beside the
 *   kiosk cookie with the same Domain, Path and lifetime, but readable by the
 *   page. `localStorage` is per origin and only `/login`, `/kiosk` and
 *   Settings → Crew run the probe, so on an org subdomain the flag is never
 *   set; the cookie reaches every host the kiosk cookie does.
 *
 * Device-level, like the theme, so `clearAuthScopedStorage` leaves both alone.
 */

const KIOSK_BROWSER_KEY = "oxy_kiosk_browser";
const KIOSK_HINT_COOKIE = "oxy_kiosk_hint";

/** Record that a probe just answered "bound". */
export function rememberKioskBrowser(): void {
  try {
    localStorage.setItem(KIOSK_BROWSER_KEY, "1");
  } catch (error: unknown) {
    // Storage disabled (private browsing, quota). The hint cookie still
    // switches the check on where the server set it.
    console.warn("Could not record that this browser is a kiosk", error);
  }
}

/** This browser stopped being a kiosk: it left kiosk mode, or its kiosk was revoked from it. */
export function forgetKioskBrowser(): void {
  try {
    localStorage.removeItem(KIOSK_BROWSER_KEY);
  } catch (error: unknown) {
    console.warn("Could not forget that this browser was a kiosk", error);
  }
}

export function isRememberedKioskBrowser(): boolean {
  try {
    return localStorage.getItem(KIOSK_BROWSER_KEY) === "1";
  } catch {
    return false;
  }
}

/** The server's page-readable kiosk hint is on this browser, for this host. */
export function hasKioskHintCookie(): boolean {
  try {
    return document.cookie.split(";").some((part) => part.trim() === `${KIOSK_HINT_COOKIE}=1`);
  } catch {
    return false;
  }
}

/** Either signal: worth asking the kiosk probe. */
export function mayBeKioskBrowser(): boolean {
  return isRememberedKioskBrowser() || hasKioskHintCookie();
}
