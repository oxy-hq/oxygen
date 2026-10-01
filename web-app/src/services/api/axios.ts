import axios from "axios";
import { toast } from "sonner";
import { getInjectedOrg } from "@/libs/orgSubdomain";
import { reportAssumeRequired } from "@/libs/utils/assumeRequired";
import { clearAuthScopedStorage } from "@/libs/utils/authStorage";
import { reportIdeReachable, reportIdeUnavailable } from "@/libs/utils/ideHealth";
import {
  PREVIEW_HEADER,
  previewRequestHeaders,
  readPreviewReadOnlyBody,
  reportPreviewServed
} from "@/libs/utils/preview";
import { usePaywallStore } from "@/stores/usePaywallStore";
import { apiBaseURL } from "../env";

const publicAPIPaths = [
  "/auth/google",
  "/auth/okta",
  "/auth/config",
  // Cookie→session hydration on org-subdomain boot. A 401 here just means
  // "no valid session cookie" — OrgSubdomainAuthGate owns the fallback
  // (bounce to the centralized app-host login), so the interceptor must not
  // also redirect (double-bounce) on it.
  "/auth/session",
  "/auth/magic-link/request",
  "/auth/magic-link/verify",
  // Dev sign-in refusals are the endpoint's normal vocabulary — 403 for an
  // email that isn't on the allow-list, 404 when the bypass is off — and the
  // /dev-login page renders both with the fix. Without this exemption the
  // caller also gets the generic "You don't have permission to do this."
  // toast, which is the wrong register for a sign-in attempt: nothing has
  // been denied to a *session*, the identity was simply never on the list.
  "/auth/dev-login",
  // Crew (frontline) sign-in. A wrong PIN answers 401 — the page renders it
  // inline and clears the PIN; a hard navigate to /login would wipe the
  // kiosk's roster picker mid-attempt. The other two always answer 200 but
  // belong to the same unauthenticated surface.
  "/frontline/device",
  "/frontline/roster",
  "/frontline/login",
  // Logging out with an already-expired token 401s; let AuthContext.logout's
  // own teardown + home redirect own the outcome instead of this interceptor
  // racing a redirect to /login. (Only gates the 401 handler — the request
  // interceptor still attaches the token, so a valid-token logout works.)
  "/logout",
  "/health",
  "/ready",
  "/live"
];

export const apiClient = axios.create({
  baseURL: apiBaseURL,
  // Send + receive cookies on every request. Without this, the browser
  // drops the `Set-Cookie: oxy_session=...` header on the login response,
  // and the custom-app iframe + cross-subdomain SSO have nothing to
  // attach. The bearer token in localStorage continues to work for the
  // SPA itself; the cookie is what lets bundle traffic authenticate.
  withCredentials: true
});

apiClient.interceptors.request.use(
  (config) => {
    const token = localStorage.getItem("auth_token");
    if (token) {
      config.headers.Authorization = token;
    }
    // Pinned to a preview: say so on every request, beside the `?branch=` the
    // call site already sends. Absent on live pages and in the IDE, which
    // sends the same `?branch=` for its (editable) working copy.
    for (const [name, value] of Object.entries(previewRequestHeaders())) {
      config.headers[name] = value;
    }
    return config;
  },
  (error) => {
    return Promise.reject(error);
  }
);

// Flight-dedupe 403 toasts: bursts of parallel mutations shouldn't stack
// multiple identical permission-denied toasts.
const makeResponseErrorHandler = () => {
  let last403At = 0;
  return (error: {
    response?: {
      status?: number;
      headers?: Record<string, string | undefined>;
      data?: {
        code?: string;
        status?: "incomplete" | "unpaid" | "canceled";
        contact_required?: boolean;
      };
    };
    config?: { url?: string };
    message?: string;
  }) => {
    const status = error.response?.status;
    const url = error.config?.url ?? "";

    reportPreviewServed(error.response?.headers?.[PREVIEW_HEADER]);

    // Pinned to a preview, anything that runs or changes something is refused
    // with a message written for the person. Say it once, plainly, whichever
    // surface made the call — most call sites toast a fixed "Failed to …" that
    // would otherwise be all anyone sees. The error's own `message` is swapped
    // for the server's too, so a call site that shows `error.message` repeats
    // the real reason rather than "Request failed with status code 409".
    const previewReadOnly = readPreviewReadOnlyBody(status, error.response?.data);
    if (previewReadOnly) {
      toast.error(previewReadOnly, { id: "preview-read-only" });
      error.message = previewReadOnly;
    }

    if (status === 401 && !publicAPIPaths.includes(url)) {
      // Sweep persisted per-user state alongside the token; a hard navigate
      // to /login lets Zustand rehydrate the next user from a clean slate.
      clearAuthScopedStorage();
      // On a bare org subdomain auth is centralized on the app host, so
      // re-auth must go there (one OAuth callback for the whole fleet), then
      // `return_to` bounces back. A local `/login` would be re-bounced by the
      // backend anyway, but going direct avoids a stale-cookie redirect loop.
      const org = getInjectedOrg();
      if (org?.appBaseUrl) {
        window.location.href = `${org.appBaseUrl}/login?return_to=${encodeURIComponent(
          window.location.href
        )}`;
      } else {
        window.location.href = "/login";
      }
    }

    if (status === 403 && !publicAPIPaths.includes(url)) {
      // Staff hitting a tenant workspace without a live assume-role session:
      // the backend stamps `x-oxy-assume-required: <org_id>`. That's a policy
      // boundary, not a missing permission, and it has a way through — so it
      // gets the assume prompt rather than the generic denial toast.
      const assumeOrgId = error.response?.headers?.["x-oxy-assume-required"];
      if (assumeOrgId) {
        const body = error.response?.data as
          | { org_name?: string | null; message?: string | null }
          | undefined;
        reportAssumeRequired(assumeOrgId, body?.org_name ?? null, body?.message ?? null);
      } else {
        const now = Date.now();
        if (now - last403At > 1500) {
          last403At = now;
          toast.error("You don't have permission to do this.");
        }
      }
    }

    if (status === 402 && error.response?.data?.code === "subscription_required") {
      const billingStatus = error.response.data.status ?? "incomplete";
      const contactRequired = error.response.data.contact_required !== false;
      usePaywallStore.getState().show(billingStatus, contactRequired);
    }

    // The developer-environment singleton is unreachable: the serving replica
    // stamps `x-oxy-required-role: ide` on its 502 so this is distinguishable
    // from a generic gateway error. Surface the global IDE-unavailable banner.
    if (status === 502 && error.response?.headers?.["x-oxy-required-role"] === "ide") {
      reportIdeUnavailable(url);
    }

    return Promise.reject(error);
  };
};

apiClient.interceptors.response.use((response) => {
  // A response served by the IDE backend — directly, or forwarded to it by a
  // serve replica — proves it's reachable again, so retire any IDE-down banner.
  // The header is `<role>@<host>#<pid>` (e.g. `ide@box#42`), so match on the
  // role prefix, not the whole value.
  const servedBy = (response.headers as Record<string, string | undefined> | undefined)?.[
    "x-oxy-served-by"
  ];
  if (servedBy?.split("@")[0] === "ide") reportIdeReachable();
  // Served from a preview: the confirmation the preview bar waits for.
  reportPreviewServed(
    (response.headers as Record<string, string | undefined> | undefined)?.[PREVIEW_HEADER]
  );
  return response;
}, makeResponseErrorHandler());
