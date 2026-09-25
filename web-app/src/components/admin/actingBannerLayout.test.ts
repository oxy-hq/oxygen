/**
 * While acting as a tenant, `body.is-acting` pushes `.root` down by the banner and
 * shrinks it by the same amount. Every full-viewport container *inside* `.root`
 * must shrink with it: one left at `100svh` overflows `.root` by the banner's
 * height, `overflow: hidden` clips the overflow, and what gets clipped is the
 * bottom of the rail — the user menu.
 *
 * This reads the stylesheet rather than measuring a layout because the unit
 * runner has no browser; the layout itself was reproduced in Chromium when the
 * fix landed (menu bottom at viewport + 32px, i.e. exactly the banner).
 */
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(path.resolve(__dirname, "../../styles/shadcn/index.css"), "utf8");

function ruleBody(selector: string): string | undefined {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return css.match(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`))?.[1];
}

describe("acting banner reserves its height", () => {
  it("caps the sidebar wrapper's min-h-svh by the banner height", () => {
    // `SidebarProvider` renders `data-slot="sidebar-wrapper"` with `min-h-svh`,
    // and wraps every workspace, admin and partner page.
    const body = ruleBody('body.is-acting [data-slot="sidebar-wrapper"]');
    expect(body).toBeDefined();
    expect(body).toMatch(/min-height:\s*calc\(100svh - var\(--acting-banner-h\)\)/);
  });
});
