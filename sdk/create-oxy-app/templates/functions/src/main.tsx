// The SDK's three stylesheets, in the order the SDK gives: the theme (every
// token, both modes), the shell, then the desk pieces (`@oxy-hq/sdk/ui`). The
// app's own CSS comes last, so an unlayered rule of its own wins a tie.
import "@oxy-hq/sdk/theme.css";
import "@oxy-hq/sdk/shell.css";
import "@oxy-hq/sdk/ui.css";
import "./index.css";

import { NotificationsBell, OxyAppProvider } from "@oxy-hq/sdk";
import { OxyShell } from "@oxy-hq/sdk/shell";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";

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

// <OxyShell> is the dock and the top bar: the same icon rail Oxygen's home page
// shows, with every app the viewer can open, and Oxygen's top bar (breadcrumb,
// clock, Ask Oxygen, the light/dark switch). Each app opens in this tab, so
// moving between your org's apps feels like one product. With no Oxygen behind
// it (`pnpm dev` without a backend) the rail stays out of the way and the app
// renders on its own.
//
// <NotificationsBell /> is the person's inbox for THIS app: what your functions
// send with `ctx.notify`. It renders nothing until the inbox answers, so it is
// safe before your app sends anything.
const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("missing #root");

createRoot(rootEl).render(
  <StrictMode>
    <OxyAppProvider manifestOptions={manifestOptions}>
      <OxyShell topBarExtra={<NotificationsBell />}>
        <App />
      </OxyShell>
    </OxyAppProvider>
  </StrictMode>
);
