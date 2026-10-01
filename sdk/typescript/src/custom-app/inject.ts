// Runtime app-config injected into the browser by oxy when it serves
// a custom-app bundle's HTML. Lets a single bundle serve any
// registered app without having `(orgId, projectId)` baked in at
// build time — see
// `crates/app/src/server/api/custom_apps_serve.rs::inject_app_config`
// on the server side.

/**
 * Shape of `window.__OXY_APP__` written by oxy at serve time.
 * Consumed by `loadCustomAppManifest` as the authoritative identity
 * source (overrides any hints in `oxy-app.json`).
 */
export interface OxyInjectedAppConfig {
  appId: string;
  slug: string;
  orgId: string;
  orgSlug: string;
  projectId: string;
  branch: string;
  /** Empty string means same-origin (the default for v2). */
  apiBaseUrl: string;
  /**
   * The app environment serving this page. Decided by the server from the
   * host (`staging--<org>--<slug>.…` is staging), never by the bundle: a build
   * is byte-identical in every environment, so this is the only way an app can
   * tell staging from production — to pick a provider's sandbox, or to label
   * itself. Absent on a server older than app environments.
   */
  environment?: OxyAppEnvironment;
}

/** An app environment: production, staging, or one engineer's dev slot. */
export type OxyAppEnvironment = "production" | "staging" | `dev-${string}`;

declare global {
  interface Window {
    __OXY_APP__?: OxyInjectedAppConfig;
  }
}

/**
 * Read the runtime app-config oxy injected at serve time. Returns
 * `undefined` outside the browser or when the global isn't set
 * (`pnpm dev` against a non-oxy server, etc. — manifest hints are
 * the fallback).
 */
export function readInjectedAppConfig(): OxyInjectedAppConfig | undefined {
  if (typeof window === "undefined") return undefined;
  return window.__OXY_APP__;
}

/**
 * The app environment serving this page (`window.__OXY_APP__.environment`).
 * Read-only: the server sets it from the host. `undefined` when oxy did not
 * serve the page, or served it before app environments existed — callers that
 * need a default should choose it deliberately rather than assume production.
 */
export function readAppEnvironment(): OxyAppEnvironment | undefined {
  return readInjectedAppConfig()?.environment;
}
