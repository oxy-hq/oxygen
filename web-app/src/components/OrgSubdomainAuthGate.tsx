import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAuth } from "@/contexts/AuthContext";
import { getInjectedOrg, redirectToCentralLogin } from "@/libs/orgSubdomain";
import { isAuthTokenExpired } from "@/libs/utils/authStorage";
import ROUTES from "@/libs/utils/routes";
import { AuthService } from "@/services/api";

// Retry transient (network / 5xx) hydration failures a few times before
// surfacing an error — a momentary blip shouldn't matter.
const MAX_ATTEMPTS = 4;

/**
 * On a bare org subdomain (`pokehouse.oxygen-hq.com`) the SSO session lives in
 * the `.oxygen-hq.com` cookie, shared across subdomains — but the SPA gates auth
 * on `localStorage`, which is per-origin and empty (or stale) on the subdomain.
 * Without this gate the SPA renders a local `/login` whose OAuth `redirect_uri`
 * is the subdomain (the provider rejects it: `redirect_uri_mismatch`).
 *
 * So before the router mounts, hydrate auth from the cookie via
 * `GET /auth/session`:
 *   - success → populate localStorage so the SPA is authenticated;
 *   - 401 (no/expired cookie) → bounce to the centralized app-host login
 *     (unless the page is a sign-in link, which signs in where it stands);
 *   - transient error → retry, then show a retry prompt (never bounce a
 *     possibly-valid session out to login over a blip).
 *
 * No-op off org subdomains and once a fresh (unexpired) token already exists.
 */
export default function OrgSubdomainAuthGate({ children }: { children: React.ReactNode }) {
  const { login } = useAuth();
  const org = getInjectedOrg();
  // Hydrate when on a subdomain and there's no usable per-origin session: no
  // user, or a missing/expired token. Presence alone isn't enough — a stale
  // token would otherwise skip hydration and 401 on the first API call.
  const needsHydration = !!org && (isAuthTokenExpired() || !safeGetItem("user"));
  const [phase, setPhase] = useState<"hydrating" | "ready" | "error">(
    needsHydration ? "hydrating" : "ready"
  );
  const started = useRef(false);

  useEffect(() => {
    if (!needsHydration || started.current) return;
    started.current = true;
    let attempts = 0;
    let timer: number | undefined;

    const attempt = () => {
      AuthService.getSession()
        .then((res) => {
          login(res.token, res.user);
          setPhase("ready");
        })
        .catch((err: { response?: { status?: number } }) => {
          // 401 = no valid session cookie → centralized app-host login. If
          // there's no app host to bounce to (local dev), fall through.
          //
          // Except on a sign-in link (`/token-login#ticket=…`), which is its
          // own login: the page redeems it right here. Bouncing it would also
          // copy the whole URL into `return_to` — fragment included — and the
          // one-time ticket would land in a query string every proxy logs.
          if (err?.response?.status === 401) {
            if (isSignInLink() || !redirectToCentralLogin()) setPhase("ready");
            return;
          }
          // Transient (network / 5xx): retry, then surface a retry prompt
          // rather than bouncing a still-valid cookie session to login.
          attempts += 1;
          if (attempts < MAX_ATTEMPTS) {
            timer = window.setTimeout(attempt, 500 * attempts);
          } else {
            setPhase("error");
          }
        });
    };
    attempt();

    return () => {
      if (timer) window.clearTimeout(timer);
    };
  }, [needsHydration, login]);

  if (phase === "hydrating") {
    return (
      <div className='flex h-full w-full items-center justify-center'>
        <Spinner className='size-6' />
      </div>
    );
  }

  if (phase === "error") {
    return (
      <div className='flex h-full w-full flex-col items-center justify-center gap-3'>
        <p className='text-muted-foreground text-sm'>
          Couldn't reach the server. Your session is still active.
        </p>
        <Button variant='outline' onClick={() => window.location.reload()}>
          Retry
        </Button>
      </div>
    );
  }

  return <>{children}</>;
}

function isSignInLink(): boolean {
  return window.location.pathname === ROUTES.AUTH.TOKEN_LOGIN;
}

function safeGetItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
