import { OxyAppProvider } from "@oxy-hq/sdk";
import { OxyShell } from "@oxy-hq/sdk/shell";
import "@oxy-hq/sdk/shell.css";
import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";

// <OxyShell> is the dock: the same icon rail Oxygen's home page shows, with
// every app the viewer can open. Each app opens in this tab, so moving between
// your org's apps feels like one product. `hideTopBar` because this app draws
// its own header; drop it to use Oxygen's (breadcrumb, clock, Ask Oxygen). With
// no Oxygen behind it (`pnpm dev` without a backend) the rail stays out of the
// way and the app renders on its own.
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <OxyAppProvider>
      <OxyShell hideTopBar>
        <App />
      </OxyShell>
    </OxyAppProvider>
  </React.StrictMode>,
);
