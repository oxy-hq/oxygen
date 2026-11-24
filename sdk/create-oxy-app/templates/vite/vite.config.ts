import { OXY_THEME_PREPAINT } from "@oxy-hq/sdk/theme";
import oxyApp from "@oxy-hq/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

// The person's light/dark mode on <html> before the first paint. Without it the
// page paints light and then flips once <OxyShell> mounts. In <head>, where
// nothing paints until the stylesheets have loaded.
function themePrepaint(): Plugin {
  return {
    name: "oxy-theme-prepaint",
    transformIndexHtml: () => [{ tag: "script", children: OXY_THEME_PREPAINT, injectTo: "head" }]
  };
}

// `oxyApp()` handles everything Oxy-specific: it validates `oxy-app.json`
// against the SDK's schema, copies it (and the icon / art it names) into the
// build output, proxies `/api` to your local oxy server in dev, and resolves
// the base path and output directory the bundle is served from.
//
// The output directory is `oxy-app.json`'s `build.outDir` (default `out`,
// which is also what `oxyc publish` ships) — set it there, not here, so the
// two cannot disagree.
//
// The base path resolves from (in order):
//   1. `OXY_APP_BASE_PATH` — `oxyc publish` sets this from the org it resolves
//      for the target environment, so the org is never hardcoded here.
//   2. `/customer-apps/<orgSlug>/<slug>/` from `oxy-app.json`, when it names
//      an `orgSlug`.
//   3. `/` — the dev fallback, fine for `pnpm dev` and a standalone build.
export default defineConfig({
  plugins: [react(), tailwindcss(), oxyApp(), themePrepaint()]
});
