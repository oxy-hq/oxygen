// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import type { CustomApp } from "@/types/apps";
import { OpenStagingButton } from "./OpenStagingButton";

afterEach(cleanup);

const app = (over: Partial<CustomApp> = {}): CustomApp =>
  ({
    id: "a",
    slug: "warehouse",
    name: "Warehouse",
    org_slug: "acme",
    staging_url: null,
    ...over
  }) as CustomApp;

const link = () => screen.queryByTestId("admin-app-open-staging");

describe("OpenStagingButton", () => {
  // No staging build yet — nothing distinct to preview.
  it("renders nothing when staging_url is null", () => {
    render(<OpenStagingButton app={app({ staging_url: null })} />);
    expect(link()).toBeNull();
  });

  // Absent is the list-response shape and the "zone can't be derived"
  // (e.g. local dev) shape — both mean the same thing here: no link.
  it("renders nothing when staging_url is absent", () => {
    render(<OpenStagingButton app={app({ staging_url: undefined })} />);
    expect(link()).toBeNull();
  });

  it("links to the staging host, in a new tab, when a distinct staging build exists", async () => {
    const url = "https://staging--acme--warehouse.customer-apps.oxygen-hq.com/";
    render(<OpenStagingButton app={app({ staging_url: url })} />);

    const anchor = link();
    expect(anchor).not.toBeNull();
    expect(anchor?.getAttribute("href")).toBe(url);
    expect(anchor?.getAttribute("target")).toBe("_blank");
    expect(anchor?.getAttribute("rel")).toBe("noreferrer");

    await userEvent.hover(anchor as HTMLElement);
    expect((await screen.findByRole("tooltip")).textContent).toContain(
      "writes are held or go to staging copies"
    );
  });
});
