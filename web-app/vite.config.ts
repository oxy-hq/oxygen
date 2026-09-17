import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { sentryVitePlugin } from "@sentry/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { visualizer } from "rollup-plugin-visualizer";
import { defineConfig, loadEnv, type Plugin } from "vite";
import { nodePolyfills } from "vite-plugin-node-polyfills";

// One identifier per build: baked into the bundle as `__APP_VERSION__` and
// emitted to /version.json so a long-lived tab can detect that a newer build
// has been deployed (src/hooks/useVersionCheck.ts) before it trips over a
// missing chunk. CI can pin it via VITE_APP_VERSION; otherwise it combines the
// package version (human-readable) with the build timestamp — the timestamp is
// what keeps two deploys of the same package version distinguishable.
const pkg = JSON.parse(readFileSync(resolve(import.meta.dirname, "package.json"), "utf-8")) as {
  version: string;
};
const BUILD_ID = process.env.VITE_APP_VERSION || `${pkg.version}+${new Date().toISOString()}`;

// Sourcemaps exist only to be uploaded to Sentry (`oxygen-intelligence/oxy-web`).
// A stable release build in public-release.yaml carries SENTRY_AUTH_TOKEN: it
// emits hidden maps, uploads them, and the plugin deletes them (in a `finally`,
// so after a failed upload too) before `dist/` is embedded in the binary. Every
// other build (local, CI, edge, a repo without the secret) keeps maps off and
// the plugin off.
const UPLOAD_SOURCEMAPS = Boolean(process.env.SENTRY_AUTH_TOKEN);

const emitVersionJson = (): Plugin => ({
  name: "oxy-emit-version-json",
  apply: "build",
  generateBundle() {
    this.emitFile({
      type: "asset",
      fileName: "version.json",
      source: JSON.stringify({ version: BUILD_ID })
    });
  }
});

// Shared dependency configuration for both dev optimization and production chunking
const dependencies = {
  // Core React runtime only — must be alone so Rollup generates a proper
  // cross-chunk import from react-dom, ensuring react initializes first.
  reactCore: ["react"],

  // React DOM and router — separate chunk so Rollup enforces load order
  reactDom: ["react-dom"],

  // React UI components - commonly used together
  reactUI: [
    "react-router-dom",
    "react-error-boundary",
    "react-resize-detector",
    "react-resizable-panels",
    "react-hotkeys-hook",
    "react-hook-form"
  ],

  // Radix UI components - heavy UI library, separate chunk
  radixUI: [
    "@radix-ui/react-alert-dialog",
    "@radix-ui/react-avatar",
    "@radix-ui/react-checkbox",
    "@radix-ui/react-collapsible",
    "@radix-ui/react-context-menu",
    "@radix-ui/react-dialog",
    "@radix-ui/react-dropdown-menu",
    "@radix-ui/react-label",
    "@radix-ui/react-popover",
    "@radix-ui/react-select",
    "@radix-ui/react-separator",
    "@radix-ui/react-slot",
    "@radix-ui/react-switch",
    "@radix-ui/react-tabs",
    "@radix-ui/react-toggle",
    "@radix-ui/react-toggle-group",
    "@radix-ui/react-tooltip",
    "@radix-ui/react-visually-hidden"
  ],

  // Code editors - large, feature-specific chunk
  editors: ["@monaco-editor/react", "monaco-editor", "monaco-yaml"],

  // Data visualization - large but specific use case
  visualization: ["echarts", "@xyflow/react", "elkjs"],

  // Content processing - markdown and syntax highlighting
  contentProcessing: [
    "react-markdown",
    "rehype-raw",
    "rehype-sanitize",
    "remark-directive",
    "remark-gfm",
    "unist-util-visit",
    "react-syntax-highlighter"
  ],

  // Data management - state, queries, tables
  dataVendor: [
    "@tanstack/react-query",
    "@tanstack/react-virtual",
    "zustand",
    "@duckdb/duckdb-wasm"
  ],

  // UI utilities and theming
  uiUtils: [
    "class-variance-authority",
    "clsx",
    "tailwind-merge",
    "sonner",
    "next-themes",
    "tailwindcss-animate",
    "lucide-react"
  ],

  // Animations and interactions
  animations: [
    "@lottiefiles/react-lottie-player",
    "@formkit/auto-animate",
    "react-day-picker" // Date picker has its own animations
  ],

  dateUtils: ["dayjs", "date-fns"],

  dataProcessing: ["lodash", "papaparse", "nunjucks", "yaml"],

  // Small utilities and helpers
  utils: [
    "usehooks-ts",
    "js-cookie" // Browser storage utility
  ],

  // State persistence
  persistence: [
    "persist-and-sync" // Added - missing from original config
  ],

  // Network and external services
  network: ["axios", "@microsoft/fetch-event-source"],

  // Development and polyfills - less critical, can be lazy loaded
  dev: ["dotenv", "memfs"]
};

// Flatten all dependencies for optimizeDeps.include
const allDependencies = Object.values(dependencies).flat();

// Build a map of package name → chunk name for function-based manualChunks.
// The function form is required so Rollup matches actual module file paths and
// always emits proper ES import statements between chunks (guaranteeing that
// react initializes before any consumer chunk runs).
const chunkNameMap: Record<string, string> = {};
const chunkEntries: [string, string[]][] = [
  ["react-vendor", dependencies.reactCore],
  ["react-dom-vendor", dependencies.reactDom],
  ["react-ui", dependencies.reactUI],
  ["radix-ui", dependencies.radixUI],
  ["editor-vendor", dependencies.editors],
  ["visualization", dependencies.visualization],
  ["content-processing", dependencies.contentProcessing],
  ["data-vendor", dependencies.dataVendor],
  ["ui-utils", dependencies.uiUtils],
  ["animations", dependencies.animations],
  ["date-utils", dependencies.dateUtils],
  ["data-processing", dependencies.dataProcessing],
  ["utils-vendor", dependencies.utils],
  ["persistence", dependencies.persistence],
  ["network-vendor", dependencies.network],
  ["dev-vendor", dependencies.dev]
];
for (const [chunkName, pkgs] of chunkEntries) {
  for (const pkg of pkgs) {
    chunkNameMap[pkg] = chunkName;
  }
}

function manualChunks(id: string): string | undefined {
  if (!id.includes("/node_modules/")) return undefined;
  for (const [pkg, chunk] of Object.entries(chunkNameMap)) {
    // Match /node_modules/pkg/ or /node_modules/@scope/pkg/
    if (id.includes(`/node_modules/${pkg}/`)) {
      return chunk;
    }
  }
  return undefined;
}

// https://vitejs.dev/config/
// Dev-server port and backend proxy target are overridable via env so more than
// one dev server can run at once without colliding on :5173 / :3000. Set
// OXY_DEV_PORT to move the Vite server and OXY_DEV_PROXY_TARGET to point /api +
// /customer-apps at a backend on another port. These are read from the repo-root
// .env (the same file the Rust backend loads) so a checkout only sets them once.
export default defineConfig(({ mode }) => {
  const rootEnv = loadEnv(mode, resolve(import.meta.dirname, ".."), "");
  const DEV_PORT = Number(rootEnv.OXY_DEV_PORT) || 5173;
  const DEV_PROXY_TARGET = rootEnv.OXY_DEV_PROXY_TARGET || "http://localhost:3000";
  // Origin of the OAuth bounce proxy (the single port registered with Google).
  // When set, the Google sign-in flow sends this as the redirect_uri and the
  // proxy bounces the callback back to this instance. Empty → per-origin flow.
  const OAUTH_PROXY_ORIGIN = rootEnv.OXY_OAUTH_PROXY_ORIGIN || "";

  return {
    base: "/",
    define: {
      __OXY_OAUTH_PROXY_ORIGIN__: JSON.stringify(OAUTH_PROXY_ORIGIN),
      __APP_VERSION__: JSON.stringify(BUILD_ID)
    },
    optimizeDeps: {
      include: allDependencies,
      // Exclude packages that you're actively developing or that cause issues when pre-bundled
      exclude: [
        // These are optional Node.js-only peer dependencies of memfs that don't exist in browser
        "@jsonjoy.com/fs-node",
        "@jsonjoy.com/fs-node-utils"
      ]
    },
    resolve: {
      alias: [
        { find: "@", replacement: resolve(import.meta.dirname, "./src") },
        { find: "styled-system", replacement: resolve(import.meta.dirname, "./styled-system") },
        { find: "elkjs", replacement: "elkjs/lib/elk.bundled.js" },
        // react-syntax-highlighter@16 imports "refractor/lib/core|all" and "refractor/lang/*.js"
        // directly, but refractor@5 only exposes these via its exports map as
        // "refractor/core", "refractor/all", and "refractor/<lang>" (no lib/ or lang/ prefix).
        { find: "refractor/lib/core", replacement: "refractor/core" },
        { find: "refractor/lib/all", replacement: "refractor/all" },
        { find: /^refractor\/lang\/([^/]+)\.js$/, replacement: "refractor/$1" },
        // monaco-yaml pulls in monaco-worker-manager@2 (last published 2022, so
        // not fixable upstream), whose worker.js imports
        // "monaco-editor/esm/vs/editor/editor.worker.js". monaco-editor 0.56
        // rewrote its exports map to:
        //     "./*.js": "./esm/vs/*.js",
        //     "./*":    "./esm/vs/*.js"
        // so that legacy spelling resolves to esm/vs/esm/vs/... and no longer
        // exists. "monaco-editor/editor/editor.worker.js" matches the "./*.js"
        // entry and lands on the same file, which still exports `initialize`.
        // Anchored to that one specifier on purpose: a wildcard would also
        // silently redirect a path monaco had MOVED rather than merely remapped.
        {
          find: "monaco-editor/esm/vs/editor/editor.worker.js",
          replacement: "monaco-editor/editor/editor.worker.js"
        }
      ]
    },
    plugins: [
      react(),
      tailwindcss(),
      emitVersionJson(),
      nodePolyfills({
        overrides: {
          // Since `fs` is not supported in browsers, we can use the `memfs` package to polyfill it.
          fs: "memfs"
        }
      }),
      !process.env.CI &&
        visualizer({
          open: true,
          filename: "bundle-report.html",
          gzipSize: true,
          brotliSize: true
        }),
      sentryVitePlugin({
        disable: !UPLOAD_SOURCEMAPS,
        // infrastructure sentry/oxy: variables.tf `organization`, projects.tf `oxy_web`.
        org: process.env.SENTRY_ORG || "oxygen-intelligence",
        project: process.env.VITE_SENTRY_PROJECT || "oxy-web",
        authToken: process.env.SENTRY_AUTH_TOKEN,
        // The same name the bundle reports: VITE_SENTRY_RELEASE, `oxy@X.Y.Z`.
        release: { name: process.env.VITE_SENTRY_RELEASE },
        sourcemaps: { filesToDeleteAfterUpload: ["./dist/**/*.map"] },
        // A Sentry outage, a bad token or a rejected option must not fail a
        // release: warn and ship. The plugin already tolerates a failed upload on
        // its own (`handleRecoverableError(e, false)`); this handler is what also
        // keeps a rejected option set from throwing, and names the cause.
        errorHandler: (err) => {
          console.warn(
            `[sentry-vite-plugin] sourcemap upload failed; the build continues: ${err.message}`
          );
        }
      })
    ],
    publicDir: "public",
    clearScreen: false,
    server: {
      port: DEV_PORT,
      // Accept requests from any *.trycloudflare.com subdomain so cloudflared
      // quick tunnels (used for Slack webhook testing) aren't rejected.
      allowedHosts: [".trycloudflare.com"],
      // https: {
      //   key: "../localhost+2-key.pem",
      //   cert: "../localhost+2.pem",
      // },
      // Proxy everything the backend serves (all under /api/* — see
      // crates/app/src/cli/commands/serve.rs where the api_router is nested
      // under /api) so Slack webhooks, OAuth callbacks, and API calls routed
      // through the dev tunnel all reach the Rust backend on :3000.
      // Loopback by ADDRESS, not by name.
      //
      // Vite's default is `localhost`, which Node resolves to the first address
      // the resolver returns — on macOS that is `::1`, so the server binds IPv6
      // loopback only. A browser that reaches for 127.0.0.1 then gets a
      // connection refused while `curl` works, which is why it reads as "the
      // app is broken" rather than "the bind is wrong". Naming the address
      // removes the resolver from the decision.
      //
      // **Not `true`.** That binds every interface, and a dev box holds real
      // cloud credentials — so exposing it to a café, hotel or office network
      // should be something someone types, which is what `vite --host` is, and
      // not a default. `OXY_DEV_HOST=true` opts in for testing on a phone.
      //
      // Second layer, not the only one: the backend already refuses to vend the
      // INFERRED `/dev-login` roster to non-loopback callers. An explicitly-set
      // `OXY_DEV_LOGIN_EMAILS` is a deliberate choice to serve other hosts, and
      // this keeps that choice from being made by a bind default nobody read.
      host: rootEnv.OXY_DEV_HOST || "127.0.0.1",
      proxy: {
        // `xfwd: true` for the same reason as /customer-apps below, but for a
        // different gate: oxy's customer-app data gate (check_custom_app_gates)
        // checks the browser's Origin against an allowlist of canonical dev
        // ports (5173/5174, 3000-3005) and, failing that, against the request's
        // X-Forwarded-Host/Host. `changeOrigin: true` rewrites Host to the
        // backend's, so on a non-canonical OXY_DEV_PORT (e.g. 5273) neither
        // check matches and SDK bundles calling /api/projects/{id}/semantic/*
        // get 403 "origin not allowed". Forwarding the real host makes
        // is_self_origin match whatever port this dev server is on.
        "/api": {
          target: DEV_PROXY_TARGET,
          changeOrigin: true,
          xfwd: true
        },
        // Customer-app bundles live at the same origin as the SPA in
        // production (e.g. https://app.oxygen-hq.com/customer-apps/<uuid>/). Locally
        // the SPA is on :5173 and oxy is on :3000, so forward those requests
        // through to oxy; otherwise vite's catch-all hands back the SPA
        // index.html and the bundle never renders.
        //
        // `xfwd: true` adds X-Forwarded-Host/Proto so oxy's redirect_to_login
        // in custom_apps_serve.rs can build a return_to URL pointing back at
        // the SPA host (:5173), not at oxy's own host (:3000).
        "/customer-apps": {
          target: DEV_PROXY_TARGET,
          changeOrigin: true,
          xfwd: true
        }
      },
      // Enable faster dependency pre-bundling during development
      fs: {
        // Allow serving files from one level up to the project root
        allow: [".."]
      },
      // Warm up frequently used files
      warmup: {
        clientFiles: [
          "./src/main.tsx",
          "./src/App.tsx",
          "./src/components/**/*.tsx",
          "./src/pages/**/*.tsx"
        ]
      }
    },
    build: {
      target: "es2020",
      // Off (memory) except in a build that uploads them; see UPLOAD_SOURCEMAPS.
      // "hidden": no `sourceMappingURL` comment points at a map, and the plugin
      // deletes the files after upload.
      sourcemap: UPLOAD_SOURCEMAPS ? "hidden" : false,
      // Increase chunk size warning limit (500kb)
      chunkSizeWarningLimit: 500,
      rollupOptions: {
        output: {
          manualChunks
        }
      }
    }
  };
});
