// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it } from "vitest";
import type { CustomApp } from "@/types/apps";
import type { FleetColumn } from "../../fleetColumns";
import { FleetRow } from "./FleetRow";
import { NOT_FROM_CI_DETAIL } from "./NotFromCiBadge";

afterEach(cleanup);

const app = (over: Partial<CustomApp> = {}): CustomApp =>
  ({
    id: "a",
    slug: "warehouse",
    name: "Warehouse",
    org_slug: "acme",
    published_at: "2026-09-19T17:30:27Z",
    last_active_at: null,
    ...over
  }) as CustomApp;

const PUBLISHED: FleetColumn[] = [{ id: "published", label: "Published" }];

const renderRow = (a: CustomApp) =>
  render(
    <MemoryRouter>
      <FleetRow app={a} status={null} columns={PUBLISHED} health={undefined} storage={undefined} />
    </MemoryRouter>
  );

const badge = () => screen.queryByTestId("admin-apps-fleet-not-from-ci");

describe("FleetRow — Not from CI", () => {
  // The incident this exists for: the one app nobody published from CI was the one
  // nobody republished when a platform release required it.
  it("flags a live build a person published, and says what to do about it", async () => {
    renderRow(app({ live_published_via: "person" }));
    expect(badge()?.textContent).toBe("Not from CI");

    await userEvent.hover(badge() as HTMLElement);
    expect((await screen.findByRole("tooltip")).textContent).toBe(NOT_FROM_CI_DETAIL);
  });

  // `ci` is the path we want; absent/null is "nothing to judge" (nothing live, or a
  // build older than the publisher column). Neither may borrow the finding.
  it.each([
    ["ci", "ci" as const],
    ["null", null],
    ["absent", undefined]
  ])("says nothing when the live build's publisher is %s", (_, via) => {
    renderRow(app({ live_published_via: via }));
    expect(badge()).toBeNull();
  });

  it("says nothing on a draft — it serves nobody", () => {
    renderRow(app({ published_at: null, live_published_via: "person" }));
    expect(screen.queryByText("Draft")).not.toBeNull();
    expect(badge()).toBeNull();
  });
});
