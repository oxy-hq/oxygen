// Shell bootstrap data for the wired OxyShell. One bundle-gated request:
//
//   GET /api/projects/:id/shell-context
//
// The server builds every URL in the response (`apps[].url`, `links.*`,
// `logo_url`) because only it knows which host scheme is in play — an org
// subdomain serves apps at `/a/<slug>/` same-origin, while a dedicated
// custom-app subdomain needs absolute product-host URLs. Never derive
// product URLs client-side: the shell-context endpoint is the authority.

import * as React from "react";
import type { OxyReach } from "../custom-app/function-context";
import { getOxyAppLogger } from "../custom-app/logger";
import { useOxyApp } from "../custom-app/react";

export interface ShellContextApp {
  id: string;
  name: string;
  slug: string;
  /** Ready-to-navigate URL (relative on the same host, absolute across hosts). */
  url: string;
  icon_url?: string | null;
  /** Agent ref the app's Ask Oxygen panel binds to (manifest `ask.agent`).
   *  Absent → the shell hides the Ask surface for this app. */
  default_agent?: string | null;
  /** Composer chips for the Ask panel (manifest `ask.suggestedQuestions`). */
  suggested_questions?: string[];
}

export interface ShellContextData {
  workspace: { id: string; name: string };
  org: { id: string; slug: string; name: string };
  /** Workspace logo endpoint URL, or null when no logo is configured. */
  logo_url: string | null;
  /** Published custom apps in this workspace, including the current one. */
  apps: ShellContextApp[];
  /** Product-surface navigation targets, host-aware. `settings` opens the
   *  Unified Settings Dialog via the SPA's `?settings=<section>` deep link
   *  (optional: absent on servers older than the link). */
  /** `login` is the product's login page, for `signOut()`; absent on a server that predates it. */
  links: { home: string; threads: string; settings?: string; login?: string };
  /** Viewer display identity; null on unauthenticated/local modes. */
  user: ShellContextUser | null;
}

/**
 * The viewer, for display. `id`, `kind` and `reach` arrive on servers that
 * carry the operating graph (2026-09-07 on) and are absent before — read them
 * through `useIdentity()`, which fills the gaps with the fail-closed answer.
 * None of this is a gate: the function's `ctx.user` is.
 */
export interface ShellContextUser {
  name: string;
  /** Empty for a frontline worker, who has no mailbox. */
  email: string;
  picture?: string | null;
  id?: string;
  /** `member` holds an org membership; `frontline` is a crew member. */
  kind?: "member" | "frontline";
  reach?: OxyReach | null;
}

export interface UseShellContextResult {
  data: ShellContextData | null;
  loading: boolean;
  error: Error | null;
}

/**
 * Fetch the shell bootstrap payload for the current custom app. Must be
 * called inside `<OxyAppProvider>`. Failure is non-fatal by design: the
 * shell degrades to chrome-less rendering (older servers don't have the
 * endpoint), so errors are surfaced on the result, never thrown.
 */
export function useShellContext(): UseShellContextResult {
  const { projectId, appId, fetcher } = useOxyApp();
  const [state, setState] = React.useState<UseShellContextResult>({
    data: null,
    loading: true,
    error: null
  });

  React.useEffect(() => {
    if (!projectId) {
      // No project identity (dev without a server / manifest hint) — nothing
      // to fetch; settle immediately so the shell degrades instead of
      // spinning forever.
      setState({ data: null, loading: false, error: new Error("no projectId resolved") });
      return;
    }
    let cancelled = false;
    // The bundle names its own app so the viewer's app-admin standing can be
    // part of the display reach; an older server ignores the parameter.
    const query = appId ? `?app=${encodeURIComponent(appId)}` : "";
    fetcher(`/api/projects/${projectId}/shell-context${query}`, { method: "GET" })
      .then(async (resp) => {
        if (!resp.ok) {
          throw new Error(`shell-context failed: HTTP ${resp.status}`);
        }
        const data = (await resp.json()) as ShellContextData;
        if (!cancelled) setState({ data, loading: false, error: null });
      })
      .catch((e: unknown) => {
        const error = e instanceof Error ? e : new Error(String(e));
        getOxyAppLogger().log("warn", "shell-context unavailable, rendering without chrome", {
          error: error.message
        });
        if (!cancelled) setState({ data: null, loading: false, error });
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, appId, fetcher]);

  return state;
}

/** What `useIdentity()` returns. */
export interface ViewerIdentity {
  id: string | null;
  name: string;
  /** `null` for a frontline worker — the crew signal, as in `ctx.user.email`. */
  email: string | null;
  picture: string | null;
  kind: "member" | "frontline" | null;
  /** Display-only reach; the fail-closed answer until the server says otherwise. */
  reach: OxyReach;
  loading: boolean;
}

const NOWHERE: OxyReach = { everywhere: false, via: null, locations: [] };

/**
 * The viewer, in one shape, on every server version: who is signed in, the
 * crew-or-office signal, and where they work — for a greeting, a "your
 * store" default, or hiding a tab. Never for authorization: a browser can
 * edit what it renders, and the function's `ctx.user.reach` is the gate.
 */
export function useIdentity(): ViewerIdentity {
  const { data, loading } = useShellContext();
  const u = data?.user ?? null;
  return {
    id: u?.id ?? null,
    name: u?.name ?? "",
    email: u?.email ? u.email : null,
    picture: u?.picture ?? null,
    kind: u?.kind ?? null,
    reach: u?.reach ?? NOWHERE,
    loading
  };
}

/**
 * Where a signed-out person lands: the product's login page, with this app
 * as the place to come back to.
 *
 * `loginUrl` is `links.login` from the shell context — the product host's,
 * which a custom-app subdomain cannot derive — and `/login` on the current
 * origin when the context is unavailable or the server predates the field.
 * `returnTo` is the app's own address, which it knows legitimately.
 */
export function signOutUrl(loginUrl: string | null | undefined, returnTo: string): string {
  const base = loginUrl?.trim() ? loginUrl.trim() : "/login";
  const sep = base.includes("?") ? "&" : "?";
  return `${base}${sep}return_to=${encodeURIComponent(returnTo)}`;
}

/**
 * End the platform session, then leave for the login page.
 *
 * `GET /api/logout` clears the session cookie server-side. That is the same
 * cookie a crew member's PIN session rides, so on an enrolled tablet the login
 * page comes back as "Who's on shift?" and returns to the app after the next
 * PIN; elsewhere it is the ordinary sign-in. Throws when the server refused,
 * so a caller can say so rather than navigate away from a session that still
 * exists. Pass the app's `fetcher` from `useOxyApp()` so credentials ride.
 */
export async function signOut(
  fetcher: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>,
  loginUrl?: string | null,
  returnTo?: string
): Promise<void> {
  const res = await fetcher("/api/logout", { method: "GET", credentials: "include" });
  if (!res.ok) throw new Error(`sign-out refused: HTTP ${res.status}`);
  if (typeof window === "undefined") return;
  window.location.assign(signOutUrl(loginUrl, returnTo ?? window.location.href));
}
