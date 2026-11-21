// @vitest-environment node
//
// The hint renders without a DOM: a screen rendered to markup on a server, or in a node test,
// never touches `document` until a bubble shows (Inventory Management, Supply Chain and Warehouse
// render their screens this way; 2.30.0 read `document.body` on every render and threw).

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { HoverHint } from "./HoverHint";

describe("HoverHint without a DOM", () => {
  it("renders to markup in node, with no document to read", () => {
    expect(typeof (globalThis as { document?: unknown }).document).toBe("undefined");
    const html = renderToStaticMarkup(
      <HoverHint text='Ordered on 12 Nov' focusable>
        <span>PO 1042</span>
      </HoverHint>
    );
    expect(html).toContain("PO 1042");
    expect(html).toContain("Ordered on 12 Nov");
    expect(html).not.toContain('role="tooltip"');
  });
});
