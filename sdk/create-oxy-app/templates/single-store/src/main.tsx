// The SDK's three stylesheets, in the order the SDK gives: the theme (every
// token, both modes), the shell, then the desk pieces (`@oxy-hq/sdk/ui`). The
// app's own CSS comes last, so an unlayered rule of its own wins a tie.
import "@oxy-hq/sdk/theme.css";
import "@oxy-hq/sdk/shell.css";
import "@oxy-hq/sdk/ui.css";
import "./index.css";

import { OxyAppProvider } from "@oxy-hq/sdk";
import { OxyShell } from "@oxy-hq/sdk/shell";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";

// <OxyShell> is the dock and the top bar: the same icon rail Oxygen's home page
// shows, with every app the viewer can open, and Oxygen's top bar (breadcrumb,
// clock, Ask Oxygen, the light/dark switch). Each app opens in this tab, so
// moving between your org's apps feels like one product. With no Oxygen behind
// it (`pnpm dev` without a backend) the rail stays out of the way and the app
// renders on its own.
const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("missing #root");

createRoot(rootEl).render(
  <StrictMode>
    <OxyAppProvider>
      <OxyShell>
        <App />
      </OxyShell>
    </OxyAppProvider>
  </StrictMode>
);
