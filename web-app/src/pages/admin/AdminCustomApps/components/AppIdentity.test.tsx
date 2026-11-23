// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AppIdentity } from "./AppIdentity";

/**
 * Wherever the admin console names an app (the fleet table's rows, the console's
 * header), its icon leads: the manifest's picture when it has one, its monogram
 * when it does not, so a long table is scanned by picture before name.
 */

afterEach(cleanup);

const app = { name: "Store Ops", slug: "store-ops", org_slug: "acme" };

describe("AppIdentity", () => {
  it("leads with the app's icon", () => {
    render(
      <AppIdentity
        app={{ ...app, icon_url: "/customer-apps/acme/store-ops/icon.svg" }}
        data-testid='id'
      />
    );
    const mark = screen.getByTestId("id").querySelector("[data-slot='app-mark']");
    expect(mark?.tagName).toBe("IMG");
    expect(mark?.getAttribute("src")).toBe("/customer-apps/acme/store-ops/icon.svg");
    expect(mark?.className).toContain("size-8");
    expect(screen.getByText("Store Ops")).toBeTruthy();
    expect(screen.getByText("acme/store-ops")).toBeTruthy();
  });

  it("falls back to the monogram without an icon, and is larger in the console header", () => {
    render(<AppIdentity app={app} size='lg' data-testid='id' />);
    const mark = screen.getByTestId("id").querySelector("[data-slot='app-mark']");
    expect(mark?.tagName).toBe("SPAN");
    expect(mark?.textContent).toBe("SO");
    expect(mark?.className).toContain("size-12");
  });
});
