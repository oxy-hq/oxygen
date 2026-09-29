import ROUTES from "@/libs/utils/routes";

/**
 * Where "Sign in as an admin" on a kiosk lands: `/kiosk`, whatever `return_to`
 * the login URL carries.
 *
 * On a kiosk that `return_to` is almost always the kiosk's own app — the enrol
 * redirect, crew sign-out and the custom-app login bounce all set it — so
 * forwarding it put the admin straight back into the app, with no way to
 * manage the tablet from the tablet.
 *
 * Absolute, on the current origin, because every provider's post-login
 * redirect goes through `resolveReturnTo`, which asks the server
 * (`validate_return_to_url`): an http(s) URL inside the session-cookie zone —
 * or loopback, when `OXY_AUTH_ALLOW_LOCALHOST_RETURN=1` in local dev.
 */
export function kioskAdminReturnTo(): string {
  return new URL(ROUTES.KIOSK, window.location.origin).toString();
}

/**
 * A same-origin `returnTo` as the path `/dev-login?next=` takes, or `undefined`
 * for none or another origin — `next` refuses anything but a local path.
 */
export function devLoginNext(returnTo: string | undefined): string | undefined {
  if (!returnTo) return undefined;
  try {
    const url = new URL(returnTo);
    return url.origin === window.location.origin ? `${url.pathname}${url.search}` : undefined;
  } catch {
    return undefined;
  }
}
