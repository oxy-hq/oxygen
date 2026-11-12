/**
 * @oxy-hq/vite-plugin — one plugin, six behaviors.
 *
 * Drop this into a custom-app's vite.config.ts and the ~50 lines of
 * base-path / outDir / dev-proxy / manifest-copy boilerplate that
 * every app re-implements collapses to:
 *
 *   plugins: [react(), oxyApp()]
 *
 * `pnpm dev` needs NO per-app config file. Identity (org/app) comes from
 * the committed `oxy-app.json`; the projectId is resolved from oxy at
 * startup via the public build-config endpoint. The only machine-level
 * settings are optional env vars (OXY_TARGET defaults to localhost:3000;
 * OXY_TOKEN only if the target oxy requires auth) — set once in your
 * shell, never per app.
 *
 * What it does, in priority order:
 *
 *   1. Sets build.outDir — the plugin option, else the manifest's
 *      `build.outDir`, else "out" — when vite.config does not, and warns
 *      when the two disagree, because `oxyc publish` reads the manifest's.
 *   2. Resolves Vite `base` automatically:
 *        OXY_APP_BASE_PATH > "/customer-apps/<orgSlug>/<slug>/" > "/"
 *      The middle form is derived from oxy-app.json's identity
 *      fields; missing fields cleanly fall through to "/".
 *   3. Validates oxy-app.json at dev and build start against the SDK's
 *      JSON Schema (`@oxy-hq/sdk/manifest` — the one check `oxyc validate`
 *      and `oxyc publish` make). Fails hard on every error: schemaVersion
 *      != 2, v1 leakage (products/writers), a missing/malformed slug or
 *      function name, a wrong type, a `notifications` catalogue publish
 *      would refuse. Warns on a field the schema does not know. v1 errors
 *      link the migration doc.
 *   4. Copies oxy-app.json into out/ at build close, and the `icon` / `art`
 *      files it names when they sit beside it. The probe reads from the
 *      served bundle; this stops being a per-app "did you remember to put
 *      it in public/" foot-gun.
 *   5. In dev (buildStart) resolves the app's projectId from oxy's public
 *      build-config endpoint using the manifest's org/app, then injects
 *      `window.__OXY_APP__` into served HTML (transformIndexHtml) so the
 *      SDK has identity in `pnpm dev` with no config file. OXY_* env vars
 *      still override any field for advanced setups. A manifest with no
 *      `orgSlug` is told exactly which flag fills the gap.
 *   6. Defaults `server.proxy["/api"]` to OXY_TARGET (or OXY_BASE_URL,
 *      default http://localhost:3000) and, when OXY_TOKEN is set,
 *      attaches it as a bearer on proxied calls (node-side only).
 *
 * Everything else (react plugin, tailwind plugin, custom Vite
 * options) coexists — we mergeConfig rather than overwrite.
 */

import { isV1Manifest, validateOxyAppManifest } from "@oxy-hq/sdk/manifest";
import { promises as fs } from "node:fs";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { loadEnv } from "vite";
import type { Plugin, ProxyOptions, ResolvedConfig, UserConfig } from "vite";

const PLUGIN_NAME = "oxy-app";
const DEFAULT_MANIFEST = "oxy-app.json";
const DEFAULT_OUT_DIR = "out";
const DEFAULT_DEV_PROXY_TARGET = "http://localhost:3000";
// The standalone v1 → v2 migration doc was folded into
// internal-docs/customer-apps.md; bundle authors who land here from
// a v1-rejection warning find current SDK + manifest guidance under §5.
const MIGRATION_DOC_URL =
  "https://github.com/oxy-hq/oxygen-internal/blob/main/internal-docs/customer-apps.md";

export interface OxyAppPluginOptions {
  /**
   * Path to the manifest, relative to project root. Default: oxy-app.json.
   * Override if the manifest lives in a non-standard location for
   * some reason — vanishingly rare.
   */
  manifest?: string;

  /**
   * Build output directory. Default: the manifest's `build.outDir`, else
   * "out" — aligned with the admin probe, `oxyc publish` and the
   * customer-apps repo conventions. Override only if you have a strong
   * reason; the plugin will warn loudly on mismatch.
   */
  outDir?: string;

  /**
   * Dev-server proxy target for `/api/*`. Default:
   * `process.env.OXY_BASE_URL || "http://localhost:3000"`. Use when
   * dev-targeting a stubbed backend or remote oxy instance.
   */
  devProxyTarget?: string;

  /**
   * Inject `window.__OXY_APP__` shim into served HTML in dev mode.
   * Default: true. Disable for tests that want to assert against the
   * raw bundle output.
   */
  injectDevShim?: boolean;
}

/**
 * The fields this plugin READS off `oxy-app.json` — a view, not the contract.
 * Every field optional, because the file is read before `validateManifest`
 * has had its say. The contract is the SDK's `OxyAppManifest`
 * (`@oxy-hq/sdk/manifest`), whose schema `validateManifest` runs; this type
 * names only what `base`, `outDir`, the dev shim and the asset copy need, and
 * is kept local so the plugin's own `.d.mts` carries no SDK import.
 */
export interface OxyManifest {
  schemaVersion?: number;
  slug?: string;
  name?: string;
  orgSlug?: string;
  projectId?: string;
  icon?: string;
  art?: string;
  build?: { outDir?: string };
}

export default function oxyApp(opts: OxyAppPluginOptions = {}): Plugin {
  const manifestPath = opts.manifest ?? DEFAULT_MANIFEST;
  const injectDevShim = opts.injectDevShim ?? true;

  // State carried between hooks. `manifest` is populated in `config`
  // and consumed by `transformIndexHtml` + `closeBundle`.
  let manifest: OxyManifest | null = null;
  let manifestAbs: string = "";
  let resolvedConfig: ResolvedConfig | null = null;
  // Dev identity for window.__OXY_APP__. org/app come from oxy-app.json
  // (committed) by default; projectId is resolved from oxy at startup
  // (see `buildStart`) so no per-app config file is needed. Any value can
  // still be overridden by an OXY_* env var for advanced/local setups.
  let devIdentity:
    | { orgSlug: string; appSlug: string; projectId: string; branch: string }
    | null = null;
  // Dev oxy URL — defaults to localhost:3000, override with OXY_TARGET.
  // Stored so `buildStart` can resolve the project from oxy.
  let devTarget = DEFAULT_DEV_PROXY_TARGET;

  return {
    name: PLUGIN_NAME,

    // `config` hook runs before everything. We read the manifest,
    // validate it, and contribute Vite config (base, outDir, proxy).
    config(userConfig, env): UserConfig {
      // Anchor manifest path on Vite's resolved project root, not
      // `process.cwd()`. When Vite is invoked from a monorepo root
      // (e.g. `vite -c apps/x/vite.config.ts`), `process.cwd()` is
      // the monorepo root and the manifest lookup would miss the
      // app's own `oxy-app.json`. `userConfig.root` is what `vite`
      // itself uses to resolve everything else in the config.
      const root = userConfig.root ?? process.cwd();
      manifestAbs = path.resolve(root, manifestPath);
      manifest = readManifest(manifestAbs);
      if (manifest) {
        // `pnpm dev` as well as `pnpm build`: the SDK refuses the same manifest
        // at app boot, and a manifest `oxyc publish` would refuse is cheaper to
        // learn about before the first page load than after the build.
        const errors = validateManifest(manifest);
        if (errors.length > 0) {
          // Fail with a single grouped error message — throwing here aborts
          // Vite cleanly with a stack trace that points at the plugin, not a
          // 200-line YAML dump.
          throw new Error(
            `[${PLUGIN_NAME}] oxy-app.json validation failed:\n  - ${errors.join("\n  - ")}` +
              (isV1Manifest(manifest) ? `\n\nMigration guide: ${MIGRATION_DOC_URL}` : "")
          );
        }
        for (const warning of manifestWarnings(manifest)) {
          console.warn(`[${PLUGIN_NAME}] oxy-app.json: ${warning}`);
        }
      }

      // Load .env / .env.local (Vite-native) so local dev identity, target,
      // and token live in a gitignored .env.local instead of oxy-app.json.
      // Prefix "" pulls every key, not just VITE_*.
      const loaded = loadEnv(env.mode, root, "");
      const envVar = (k: string): string | undefined => {
        const v = loaded[k] ?? process.env[k];
        return v && v.trim().length > 0 ? v : undefined;
      };

      const base = resolveBase(manifest, envVar("OXY_APP_BASE_PATH"));
      devTarget =
        opts.devProxyTarget ??
        envVar("OXY_TARGET") ??
        envVar("OXY_BASE_URL") ??
        DEFAULT_DEV_PROXY_TARGET;
      // `from-proxy` is oxyc's sentinel for "the egress proxy adds the
      // credential" (a cloud network secret): there is nothing to attach.
      const rawToken = envVar("OXY_TOKEN");
      const devToken = rawToken === "from-proxy" ? undefined : rawToken;

      // Dev identity comes from oxy-app.json (committed). projectId is
      // optional here — `buildStart` resolves it from oxy when absent — so
      // no per-app .env.local is required. OXY_* env vars still override
      // for advanced/local-retarget setups.
      const orgSlug = envVar("OXY_ORG") ?? manifest?.orgSlug ?? "";
      const appSlug = envVar("OXY_APP") ?? manifest?.slug ?? "";
      const projectId = envVar("OXY_PROJECT") ?? manifest?.projectId ?? "";
      const branch = envVar("OXY_BRANCH") ?? "main";
      devIdentity = appSlug ? { orgSlug, appSlug, projectId, branch } : null;

      // Said once, with the fix, rather than as a 404 from `buildStart` that
      // names the wrong cause: without an org the dev shim has nothing to
      // look the project up by, and `oxyc publish` stops on the same gap.
      if (env.command === "serve" && injectDevShim && appSlug && !orgSlug && !projectId) {
        console.warn(
          `[${PLUGIN_NAME}] oxy-app.json has no "orgSlug" and OXY_ORG is unset, so dev cannot ` +
            "resolve the app's project and the SDK will have no identity. Add " +
            '"orgSlug": "<org>" to oxy-app.json (create-oxy-app --org <slug> writes it), ' +
            "or export OXY_ORG=<org>."
        );
      }

      // `/api` dev proxy. When OXY_TOKEN is set, attach it as a bearer on
      // proxied requests so a remote dev oxy authenticates them (local dev
      // is cross-origin, so the session cookie isn't sent). The token lives
      // in the Vite node process only — never in browser JS.
      const apiProxy: ProxyOptions = {
        target: devTarget,
        changeOrigin: true
      };
      if (devToken) {
        apiProxy.configure = (proxy) => {
          proxy.on("proxyReq", (proxyReq) => {
            proxyReq.setHeader("authorization", `Bearer ${devToken}`);
          });
        };
      }

      // outDir: the plugin option, else the manifest's `build.outDir` (what
      // `oxyc publish` tars up), else the default. A vite.config that sets its
      // own gets a warning naming both, not an error — they might have a
      // reason; they just need to know the two have to agree for a publish
      // to find the bundle.
      const { outDir, from } = resolveOutDir(opts.outDir, manifest);
      const userOutDir = userConfig.build?.outDir;
      if (userOutDir && userOutDir !== outDir) {
        console.warn(
          `[${PLUGIN_NAME}] vite.config sets build.outDir "${userOutDir}" but ${from} says "${outDir}". ` +
            "`oxyc publish` tars up the manifest's build.outDir, so set it in one place: " +
            "drop build.outDir from vite.config, or put the same value in oxy-app.json `build.outDir`."
        );
      }

      // We return a partial config; Vite mergeConfig with userConfig
      // and our return value, with user winning on conflict for keys
      // where we set defaults (so user's `base` overrides ours).
      // For `server.proxy["/api"]` we only set when absent — Vite's
      // merge does the right thing here.
      // Proxy table: the Oxy data plane (`/api`) plus this app's server-side
      // function calls (`/fn`). The `/fn` key is scoped to THIS app's base so it
      // never shadows the bundle's own assets served locally under
      // `/customer-apps/<org>/<slug>/`. Point the target at `oxyc proxy` (default
      // localhost:3000) to hit a cloud env's data with your `oxyc login` token.
      const proxy: Record<string, ProxyOptions> = { "/api": apiProxy };
      if (orgSlug && appSlug) {
        proxy[`/customer-apps/${orgSlug}/${appSlug}/fn`] = apiProxy;
      }

      const contribution: UserConfig = {
        base,
        build: {
          outDir: userOutDir ?? outDir,
          // emptyOutDir defaults to true when outDir is inside root,
          // and false otherwise. Force true so stale builds don't
          // leak past a slug rename.
          emptyOutDir: true
        },
        server: {
          proxy
        }
      };
      return contribution;
    },

    configResolved(config) {
      resolvedConfig = config;
    },

    // Dev-only: resolve the app's projectId from oxy when it isn't already
    // known, using the PUBLIC (no-auth) build-config endpoint keyed by the
    // org/app slugs from oxy-app.json. This is what lets dev run with NO
    // per-app config file — the committed manifest + this lookup supply
    // everything window.__OXY_APP__ needs. Best-effort: a failure just
    // leaves projectId empty and the shim is skipped (with a warning).
    async buildStart() {
      if (resolvedConfig?.command !== "serve" || !injectDevShim) return;
      if (!devIdentity || devIdentity.projectId || !devIdentity.appSlug) return;
      // No org, no lookup: `config` has already said which flag fills it.
      if (!devIdentity.orgSlug) return;
      const url =
        `${devTarget.replace(/\/$/, "")}/api/apps/` +
        `${encodeURIComponent(devIdentity.orgSlug)}/${encodeURIComponent(devIdentity.appSlug)}/build-config`;
      try {
        const res = await fetch(url);
        if (res.ok) {
          const data = (await res.json()) as { project_id?: string; branch?: string };
          if (data.project_id) {
            devIdentity = {
              ...devIdentity,
              projectId: data.project_id,
              branch: data.branch || devIdentity.branch
            };
            console.log(
              `[${PLUGIN_NAME}] resolved project ${data.project_id} for ${devIdentity.orgSlug}/${devIdentity.appSlug}.`
            );
          }
        } else {
          console.warn(
            `[${PLUGIN_NAME}] couldn't resolve project for ${devIdentity.orgSlug}/${devIdentity.appSlug} ` +
              `from ${devTarget} (${res.status}). Register the app in oxy (its first \`oxyc publish\` does), ` +
              "or set OXY_PROJECT=<workspace-uuid> so dev can query."
          );
        }
      } catch (err) {
        console.warn(
          `[${PLUGIN_NAME}] project resolve failed (${url}): ${String(err)}. Is OXY_TARGET reachable?`
        );
      }
    },

    // Dev-only: inject window.__OXY_APP__ identity so the SDK can call
    // oxy in `pnpm dev`. Data calls are authorized by the dev token the
    // proxy attaches (above) or, when served by oxy, the session cookie.
    transformIndexHtml: {
      order: "pre",
      handler(html) {
        if (!injectDevShim || !resolvedConfig || resolvedConfig.command !== "serve") {
          return html;
        }
        // org/app from oxy-app.json; projectId resolved from oxy in
        // `buildStart`. Without a projectId the SDK can't make /query
        // calls, so skip injection entirely.
        if (!devIdentity || !devIdentity.projectId) return html;
        // Escape `</script>` inside the stringified payload before
        // splicing it into a `<script>` block. JSON.stringify is not
        // HTML-aware — a field containing `</script>` (or `</SCRIPT>`,
        // etc.) would close the inline script tag early and dump the
        // rest of the payload into the HTML stream. Low probability
        // (these are operator-controlled slugs) but the fix is one
        // regex; mirrors `custom_apps_serve::inject_app_config`.
        const payload = JSON.stringify({
          orgSlug: devIdentity.orgSlug,
          appSlug: devIdentity.appSlug,
          projectId: devIdentity.projectId,
          branch: devIdentity.branch
        }).replace(/<\/(script)/gi, "<\\/$1");
        const tag = `<script>window.__OXY_APP__ = ${payload};</script>`;
        // Splice into <head> before any other script so it's set
        // before the bundle's entry runs.
        return html.replace(/<head>/i, `<head>\n    ${tag}`);
      }
    },

    // After Vite finishes writing out/, copy oxy-app.json next to
    // index.html, and the icon / art it names. The admin probe and the
    // launcher read from the served bundle, not from the source tree.
    async closeBundle() {
      if (!resolvedConfig || resolvedConfig.command !== "build") return;
      if (!existsSync(manifestAbs)) return;
      const outAbs = path.resolve(resolvedConfig.root, resolvedConfig.build.outDir);
      const dest = path.join(outAbs, DEFAULT_MANIFEST);
      try {
        await fs.copyFile(manifestAbs, dest);
      } catch (err) {
        // Don't fail the build — log and continue. A missing
        // manifest in out/ produces a soft probe warning, not a 500.
        console.warn(`[${PLUGIN_NAME}] failed to copy ${manifestPath} into ${dest}: ${String(err)}`);
      }
      await copyManifestAssets(manifest, path.dirname(manifestAbs), outAbs);
    }
  };
}

/**
 * The `icon` and `art` files the manifest names, into the build output when
 * they are not already there.
 *
 * Both are paths relative to the bundle root, served through the app's own
 * URL. A file under `public/` is already in `out/` — Vite copies that
 * directory — so the copy is only for one that sits beside the manifest,
 * which is where `create-oxy-app` puts them when the manifest is at the
 * root. A name that resolves to neither place is a launcher with no icon,
 * said here rather than discovered on the card.
 */
async function copyManifestAssets(
  manifest: OxyManifest | null,
  manifestDir: string,
  outAbs: string
): Promise<void> {
  for (const key of ["icon", "art"] as const) {
    const rel = manifest?.[key];
    if (typeof rel !== "string" || rel.trim() === "") continue;
    // An absolute URL or a data URI names nothing in the bundle to copy.
    if (/^(?:[a-z]+:)?\/\//i.test(rel) || rel.startsWith("data:")) continue;
    const dest = path.resolve(outAbs, rel);
    if (!dest.startsWith(outAbs + path.sep)) {
      console.warn(`[${PLUGIN_NAME}] oxy-app.json ${key} "${rel}" points outside the bundle; not copied.`);
      continue;
    }
    if (existsSync(dest)) continue;
    const src = path.resolve(manifestDir, rel);
    if (!existsSync(src)) {
      console.warn(
        `[${PLUGIN_NAME}] oxy-app.json names ${key} "${rel}" but neither the build output nor the ` +
          `directory beside the manifest has it — the launcher will show no ${key}.`
      );
      continue;
    }
    try {
      await fs.mkdir(path.dirname(dest), { recursive: true });
      await fs.copyFile(src, dest);
    } catch (err) {
      console.warn(`[${PLUGIN_NAME}] failed to copy ${key} ${rel} into ${dest}: ${String(err)}`);
    }
  }
}

/**
 * Read + parse the manifest. Returns null on any failure (missing
 * file, bad JSON) — the caller decides whether that's fatal.
 */
function readManifest(absPath: string): OxyManifest | null {
  try {
    const bytes = readFileSync(absPath, "utf-8");
    return JSON.parse(bytes) as OxyManifest;
  } catch {
    return null;
  }
}

/** Where the build goes, and which setting said so. */
export function resolveOutDir(
  option: string | undefined,
  manifest: OxyManifest | null
): { outDir: string; from: string } {
  if (option?.trim()) return { outDir: option, from: "the oxyApp() outDir option" };
  const declared = manifest?.build?.outDir;
  if (typeof declared === "string" && declared.trim()) {
    return { outDir: declared, from: "oxy-app.json build.outDir" };
  }
  return { outDir: DEFAULT_OUT_DIR, from: "the default the admin probe and oxyc publish expect" };
}

/**
 * Validate a parsed manifest: the SDK's schema and rules, as `oxyc validate`
 * and `oxyc publish` apply them. Returns a list of human-readable error
 * strings (`<path>: <message>`); empty list means the manifest is valid.
 */
export function validateManifest(m: unknown): string[] {
  return validateOxyAppManifest(m)
    .filter((issue) => issue.level === "error")
    .map((issue) => `${issue.path}: ${issue.message}`);
}

/**
 * The manifest's warnings: what publishes, but is probably a mistake — a
 * field the schema does not know, a catalogue kind no function sends, a
 * level a `critical: true` overrides.
 */
export function manifestWarnings(m: unknown): string[] {
  return validateOxyAppManifest(m)
    .filter((issue) => issue.level === "warning")
    .map((issue) => `${issue.path}: ${issue.message}`);
}

/**
 * Resolve Vite's `base` option. Priority:
 *   1. OXY_APP_BASE_PATH env (CI always sets this — keep working).
 *   2. Derived from manifest's orgSlug + slug.
 *   3. "/" (dev default, root-mounted).
 *
 * Always returns a path with a trailing slash — Vite requires that.
 */
export function resolveBase(
  manifest: OxyManifest | null,
  envBaseOverride?: string
): string {
  const envBase = envBaseOverride ?? process.env.OXY_APP_BASE_PATH;
  if (envBase && envBase.trim().length > 0) {
    return envBase.endsWith("/") ? envBase : `${envBase}/`;
  }
  if (manifest?.orgSlug && manifest?.slug) {
    return `/customer-apps/${manifest.orgSlug}/${manifest.slug}/`;
  }
  return "/";
}
