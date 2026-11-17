// @vitest-environment jsdom
//
// OxyChat draws its field and buttons with inline styles, so it reads the theme there: with
// `@oxy-hq/sdk/theme.css` the field and buttons take the theme's corners and Ask the theme's
// primary pair (brand fill and its label colour); without it, each falls back to what it drew
// before (6px corners, white on `--oxy-shell-link`, which measured 2.69:1 in dark).

import * as React from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { _resetCustomAppManifestCacheForTest } from "./manifest";
import { OxyAppProvider, OxyChat } from "./react";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  _resetCustomAppManifestCacheForTest();
  window.__OXY_APP__ = {
    appId: "app-uuid",
    slug: "test-app",
    orgId: "org-uuid",
    orgSlug: "acme",
    projectId: "proj-uuid",
    branch: "main",
    apiBaseUrl: ""
  };
  globalThis.fetch = (async () =>
    ({
      ok: true,
      status: 200,
      json: async () => ({
        schemaVersion: 2,
        name: "Test App",
        slug: "test-app",
        orgSlug: "acme",
        projectId: "proj-uuid"
      })
    }) as Response) as typeof fetch;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await React.act(async () => root.unmount());
  container.remove();
});

async function mount(): Promise<void> {
  await React.act(async () => {
    root.render(
      <OxyAppProvider>
        <OxyChat agentId='analyst' />
      </OxyAppProvider>
    );
  });
}

describe("OxyChat reads the theme", () => {
  it("draws Ask in the theme's primary pair at the theme's button corner, else what it drew before", async () => {
    await mount();
    const ask = container.querySelector<HTMLButtonElement>("button[type='submit']");
    expect(ask?.style.getPropertyValue("background")).toBe(
      "var(--button-primary-bg, var(--oxy-shell-link, #2563eb))"
    );
    expect(ask?.style.getPropertyValue("color")).toBe("var(--button-primary-fg, #ffffff)");
    expect(ask?.style.getPropertyValue("border-radius")).toBe("var(--button-radius, 6px)");
  });

  it("gives the field the theme's field corner, else 6px", async () => {
    await mount();
    const field = container.querySelector<HTMLInputElement>("input[aria-label='Question']");
    expect(field?.style.getPropertyValue("border-radius")).toBe("var(--input-radius, 6px)");
  });
});
