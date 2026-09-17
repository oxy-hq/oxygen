import { OxyAppProvider } from "@oxy-hq/sdk";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { canaryMarker } from "./marker";

// In production Oxy injects the app's base path and the SDK fetches the
// manifest from `<injected_base>/oxy-app.json`. In a standalone `pnpm dev`
// there is no injection, so the SDK would fall back to `/oxy-app.json` at the
// server root — which 404s, because Vite serves the app (and its manifest)
// under the base path. Point the loader at Vite's BASE_URL so `pnpm dev`
// resolves the manifest with no backend running. Dev-only: production keeps
// the injected-base behavior untouched.
const manifestOptions = import.meta.env.DEV
  ? { manifestUrl: `${import.meta.env.BASE_URL}oxy-app.json` }
  : undefined;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <OxyAppProvider
      manifestOptions={manifestOptions}
      // No check can run without the manifest, so say so in the marker rather
      // than leave the browser check waiting out its timeout.
      errorFallback={(err) => <main {...canaryMarker({ manifest: "failed" })}>{err.message}</main>}
    >
      <App />
    </OxyAppProvider>
  </StrictMode>
);
