import { OxyAppProvider } from "@oxy-hq/sdk";
import { OxyShell } from "@oxy-hq/sdk/shell";
import "@oxy-hq/sdk/shell.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./index.css";

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

// <OxyShell> is the dock: the same icon rail Oxygen's home page shows, with
// every app the viewer can open. Each app opens in this tab, so moving between
// your org's apps feels like one product. `hideTopBar` because this app draws
// its own header; drop it to use Oxygen's (breadcrumb, clock, Ask Oxygen). With
// no Oxygen behind it (`pnpm dev` without a backend) the rail stays out of the
// way and the app renders on its own.
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <OxyAppProvider manifestOptions={manifestOptions}>
      <OxyShell hideTopBar>
        <App />
      </OxyShell>
    </OxyAppProvider>
  </StrictMode>
);
