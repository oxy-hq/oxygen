// Manifest loader for custom-app bundles served by oxy at
// `app.oxygen-hq.com/customer-apps/<org_slug>/<app_slug>/`.
//
// The bundle commits a `public/oxy-app.json` declaring its identity
// (slug, orgSlug, projectId). This module:
//   1. Fetches that manifest at startup (cached after the first call).
//   2. Validates it with `validateOxyAppManifest` — the same schema-driven
//      check `oxyc validate`, `oxyc publish` and the Vite plugin run — and
//      throws on the first error, with its sentence.
//   3. Joins it with the runtime identity oxy injects via
//      `<script>window.__OXY_APP__=...</script>`.
//
// The TYPES live in `manifest-types.ts` (the one definition the JSON Schema is
// generated from) and are re-exported here for the root entry; the rules and
// the validator ship as `@oxy-hq/sdk/manifest` for Node tools.
//
// Bundles call `useQuery` directly for data access — there are no
// `products` or `writers` declarations in v2 manifests.

import { type OxyInjectedAppConfig, readInjectedAppConfig } from "./inject";
import { getOxyAppLogger } from "./logger";
import type { OxyAppManifest } from "./manifest-types";
import { validateOxyAppManifest } from "./manifest-validate";

export type {
  OxyAppAirwayStepManifest,
  OxyAppAskManifest,
  OxyAppBuildManifest,
  OxyAppCacheManifest,
  OxyAppEmailCapability,
  OxyAppEnabledCapability,
  OxyAppEnvDeclaration,
  OxyAppEnvironmentManifest,
  OxyAppFetchManifest,
  OxyAppFrontlineCapability,
  OxyAppFunctionManifest,
  OxyAppManifest,
  OxyAppMigrationsManifest,
  OxyAppNonProductionManifest,
  OxyAppNotifyCapability,
  OxyAppNotifyKindCapability,
  OxyAppOrgCapability,
  OxyAppOxygenCapability,
  OxyAppPerformanceManifest,
  OxyAppPlatformCapability,
  OxyAppRetentionRule,
  OxyAppRetriesManifest,
  OxyAppSecretsCapability,
  OxyAppSourceUploadsCapability,
  OxyAppStorageCapability,
  OxyAppStorageManifest,
  OxyAppWebhookManifest
} from "./manifest-types";
export type {
  NotificationLevel,
  OxyAppNotificationAction,
  OxyAppNotificationCategory,
  OxyAppNotificationKind,
  OxyAppNotificationsManifest
} from "./notification-catalog";

// ── Resolved manifest ───────────────────────────────────────────────────────

/**
 * Manifest + runtime-injected identity needed to call oxy. Callers
 * should treat this as the only source of truth for "which org/app
 * does this bundle belong to."
 */
export interface ResolvedCustomAppManifest {
  manifest: OxyAppManifest;
  /**
   * Always an empty array for v2 manifests. Kept for API compatibility;
   * callers that previously iterated product names should switch to
   * explicit `useQuery` calls.
   * @deprecated Will be removed in a future version.
   */
  productNames: string[];
  /** Org slug injected by oxy. */
  orgSlug: string;
  /** App slug injected by oxy. */
  appSlug: string;
  /**
   * The oxy server's API base URL. Empty string when oxy serves the
   * bundle itself (same-origin, the common case); a full URL only
   * when the bundle is running under a dev server proxy.
   */
  apiBaseUrl: string;
  /** App UUID; informational. */
  appId?: string;
  /**
   * Project (workspace) UUID. Injection (`window.__OXY_APP__.projectId`)
   * wins over the manifest's `projectId` field — the admin row is
   * authoritative. Manifest `projectId` is a dev-time hint used only
   * when running without a server. Used by `useQuery` to construct the
   * `/api/projects/:id/query` URL.
   */
  projectId?: string;
}

export interface LoadManifestOptions {
  /**
   * Override the URL the manifest is fetched from. Default:
   * `<injected_base>/oxy-app.json` or `/oxy-app.json`.
   * Useful for non-Next bundlers — set explicitly to wherever your
   * bundler emits static assets.
   */
  manifestUrl?: string;
}

let cached: Promise<ResolvedCustomAppManifest> | null = null;

/**
 * Load + validate the manifest. Cached after the first call so callers
 * can invoke this from every component without coordinating.
 */
export function loadCustomAppManifest(
  options: LoadManifestOptions = {}
): Promise<ResolvedCustomAppManifest> {
  if (!cached) {
    cached = fetchAndValidate(options);
  }
  return cached;
}

/** For tests: reset the cache between runs. */
export function _resetCustomAppManifestCacheForTest(): void {
  cached = null;
}

async function fetchAndValidate(options: LoadManifestOptions): Promise<ResolvedCustomAppManifest> {
  const log = getOxyAppLogger();
  const injected = readInjectedAppConfig();
  const manifestUrl = options.manifestUrl ?? defaultManifestUrl(injected);

  log.log("info", "loading manifest", {
    manifestUrl,
    injectionPresent: !!injected,
    orgSlug: injected?.orgSlug,
    appSlug: injected?.slug,
    appId: injected?.appId
  });

  const startedAt = Date.now();
  const res = await fetch(manifestUrl, { credentials: "same-origin" });
  if (!res.ok) {
    log.log("error", "manifest fetch failed", {
      manifestUrl,
      status: res.status,
      statusText: res.statusText
    });
    throw new Error(
      `Failed to load oxy-app.json from ${manifestUrl} (HTTP ${res.status}). ` +
        `The custom-app repo must commit this file alongside the bundle.`
    );
  }
  const raw = (await res.json()) as unknown;
  const manifest = validateManifest(raw, manifestUrl);

  const resolved: ResolvedCustomAppManifest = {
    manifest,
    productNames: [],
    orgSlug: injected?.orgSlug ?? "",
    appSlug: injected?.slug ?? "",
    apiBaseUrl: injected?.apiBaseUrl || "",
    appId: injected?.appId,
    projectId: injected?.projectId ?? manifest.projectId
  };
  log.log("info", "manifest ready", {
    durationMs: Date.now() - startedAt,
    schemaVersion: manifest.schemaVersion,
    slug: manifest.slug
  });
  return resolved;
}

/**
 * Default manifest URL.
 *
 * Resolution order (bundler-agnostic):
 *   1. `window.__OXY_APP__.orgSlug`/`slug` injection → the canonical
 *      `/customer-apps/<org>/<app>/oxy-app.json`. Works for every
 *      bundle oxy serves regardless of how it was built.
 *   2. `NEXT_PUBLIC_APP_BASE_PATH` env var — kept for backward compat
 *      with Next.js bundles that bake basePath at build time.
 *   3. Empty basePath → `/oxy-app.json` (only matches when running in
 *      a `vite dev` / `next dev` root mount; will 404 under oxy).
 */
function defaultManifestUrl(injected: OxyInjectedAppConfig | undefined): string {
  if (injected?.orgSlug && injected?.slug) {
    const org = encodeURIComponent(injected.orgSlug);
    const app = encodeURIComponent(injected.slug);
    return `/customer-apps/${org}/${app}/oxy-app.json`;
  }
  // No injection → bundle is running outside oxy (`pnpm dev` against
  // a local Vite, an iframe preview, etc.). Look up `/oxy-app.json`
  // at the document root; the vite-plugin's dev shim and the
  // standard `public/` convention both serve it there.
  return "/oxy-app.json";
}

// ── Validation ──────────────────────────────────────────────────────────────

/**
 * Validate a v2 manifest against the schema and throw on the first error.
 *
 * The Vite plugin's build-time gate is the primary one; this fires on a
 * standalone `pnpm dev` and on a bundle built without the plugin. Warnings
 * (an unknown field, a catalogue kind nothing sends) go to the app logger.
 *
 * At serve time, oxy's identity injection (window.__OXY_APP__) overrides
 * the manifest's orgSlug/projectId — the manifest fields are advisory.
 */
function validateManifest(raw: unknown, url: string): OxyAppManifest {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    throw new Error(`Manifest at ${url} is not a JSON object`);
  }
  const issues = validateOxyAppManifest(raw);
  const errors = issues.filter((issue) => issue.level === "error");
  if (errors.length > 0) {
    // One line, every error: the first sentence is the headline and the rest
    // follow it, so a manifest with three problems is fixed in one round.
    throw new Error(`oxy-app.json: ${errors.map((issue) => issue.message).join("; ")}`);
  }
  const log = getOxyAppLogger();
  for (const issue of issues) {
    if (issue.level === "warning")
      log.log("warn", `oxy-app.json (${issue.path}): ${issue.message}`);
  }
  return raw as OxyAppManifest;
}
