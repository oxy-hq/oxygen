import { OXY_THEME_PREPAINT } from "@oxy-hq/sdk/theme";
import oxyApp from "@oxy-hq/vite-plugin";
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

// `oxyApp()` handles base path, outDir, manifest validation, manifest
// copy, dev `/api` proxy, and the dev-time `window.__OXY_APP__` shim.
export default defineConfig({
  plugins: [react(), oxyApp(), themePrepaint()]
});
