import ROUTES from "@/libs/utils/routes";
import { AuthService } from "@/services/api";
import type { AuthResponse, OrgInfo, UserInfo } from "@/types/auth";

export const PENDING_INVITE_TOKEN_KEY = "pending_invite_token";

/**
 * A `next` destination is only followed when it is a same-origin path, so a
 * sign-in URL that carries one (`/dev-login?next=`, `/token-login#next=`)
 * can't be turned into an open redirect. Cross-origin destinations go through
 * `return_to`, which the server validates.
 *
 * Decided by resolving it, not by its prefix. `/token-login` follows the
 * result with a full page load, and a URL parser reads `\` as `/` and drops
 * tabs and newlines before it parses — so `/\host` and `/<tab>/host` both
 * start with a single slash and both leave the origin. What comes back is the
 * resolved path, query and fragment: exactly where the browser would go.
 */
export const sanitizeNextPath = (next: string | null | undefined): string | null => {
  if (!next?.startsWith("/") || next.startsWith("//")) {
    return null;
  }
  try {
    const resolved = new URL(next, window.location.origin);
    if (resolved.origin !== window.location.origin) {
      return null;
    }
    return `${resolved.pathname}${resolved.search}${resolved.hash}`;
  } catch {
    return null;
  }
};

const RETURN_TO_KEY = "oxy_post_login_return_to";

/**
 * Persist a `return_to` across an OAuth provider round-trip.
 *
 * OAuth bounces the browser off-domain (to Google/Okta/GitHub) and back to a
 * fixed callback URL, so a `return_to` query param can't ride along, and the
 * signed `state` token is reserved for CSRF. We stash it in `sessionStorage`
 * (same mechanism the CSRF state uses, and it survives the same-origin return)
 * and read it back in the callback.
 *
 * Always reflects the CURRENT attempt: an empty value clears any previously
 * stashed destination, so abandoning a login started from a custom app and
 * later signing in from a plain `/login` doesn't redirect into the stale app.
 */
export function stashReturnTo(returnTo: string | null | undefined): void {
  if (returnTo) {
    sessionStorage.setItem(RETURN_TO_KEY, returnTo);
  } else {
    sessionStorage.removeItem(RETURN_TO_KEY);
  }
}

/** Read and clear the stashed `return_to` (see {@link stashReturnTo}). */
export function consumeReturnTo(): string | null {
  const value = sessionStorage.getItem(RETURN_TO_KEY);
  if (value) {
    sessionStorage.removeItem(RETURN_TO_KEY);
  }
  return value;
}

/**
 * Resolve a post-login `return_to` into a safe destination. Returns the URL
 * only when the server confirms it's allowed (see `validateReturnTo`), so the
 * caller can `window.location.href` into it (e.g. back to a custom-app
 * subdomain). Returns `null` when there's no `return_to` or the server rejects
 * it — the caller then falls back to {@link handlePostLoginOrgs}.
 */
export async function resolveReturnTo(returnTo: string | null | undefined): Promise<string | null> {
  if (!returnTo) {
    return null;
  }
  if (await AuthService.validateReturnTo(returnTo)) {
    return returnTo;
  }
  console.warn("return_to URL rejected by server; falling back to default destination");
  return null;
}

/** Read the `return_to` query param from the current login URL, if present. */
export function returnToFromUrl(): string | null {
  return new URLSearchParams(window.location.search).get("return_to");
}

export function handlePostLoginOrgs(user: UserInfo, orgs: OrgInfo[]): string {
  const pendingInviteToken = sessionStorage.getItem(PENDING_INVITE_TOKEN_KEY);
  if (pendingInviteToken) {
    sessionStorage.removeItem(PENDING_INVITE_TOKEN_KEY);
    return ROUTES.INVITE(pendingInviteToken);
  }

  if (user.is_owner) {
    return ROUTES.ADMIN.BILLING_QUEUE;
  }

  if (orgs.length === 0) {
    // Staff standing with no membership (an App Operator, a Global Admin before
    // any tenant exists) is working in the admin console, not waiting on an
    // invite. `/onboarding` stays reachable by URL for a staff invite.
    return user.is_app_admin ? ROUTES.ADMIN.CUSTOMER_APPS : ROUTES.ONBOARDING;
  }

  return ROUTES.ROOT;
}

/** Where a sign-in page's own URL asked to go once signed in. */
export interface RequestedDestination {
  /** Cross-origin post-login destination; validated server-side. */
  returnTo?: string;
  /** Same-origin path to land on, e.g. `/ide`. Wins over the org dispatcher. */
  next?: string | null;
}

export type PostLoginDestination =
  /** A server-validated `return_to`: another origin, so a full navigation. */
  | { kind: "external"; url: string }
  /** A path on this origin. */
  | { kind: "path"; path: string };

/**
 * Where a completed sign-in goes, for the pages that take a destination from
 * their own URL (`/dev-login`, `/token-login`): a validated `return_to` wins,
 * then a same-origin `next`, then wherever the user's orgs say.
 */
export async function resolvePostLoginDestination(
  auth: Pick<AuthResponse, "user" | "orgs">,
  { returnTo, next }: RequestedDestination
): Promise<PostLoginDestination> {
  const resolved = await resolveReturnTo(returnTo);
  if (resolved) {
    return { kind: "external", url: resolved };
  }
  return {
    kind: "path",
    path: sanitizeNextPath(next) ?? handlePostLoginOrgs(auth.user, auth.orgs)
  };
}

/**
 * Leave by a full page load, replacing the current history entry — for a
 * sign-in page that must not be left with a soft navigation (see
 * `useTokenLogin`) and should not be what Back returns to.
 */
export function leaveTo(url: string): void {
  window.location.replace(url);
}
