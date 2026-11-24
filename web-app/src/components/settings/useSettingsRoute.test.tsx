// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useSettingsRoute } from "./useSettingsRoute";

vi.mock("@/stores/useCurrentOrg", () => ({
  default: (selector: (s: { org: { slug: string } }) => unknown) =>
    selector({ org: { slug: "settings-co" } })
}));

const Probe = () => {
  const { section, tab, isOpen } = useSettingsRoute();
  return <output data-testid='probe'>{`${isOpen}|${section ?? "-"}|${tab ?? "-"}`}</output>;
};

/** Mounted the way the app mounts it: under a `settings/*` splat inside an org segment. */
const at = (url: string) => {
  cleanup();
  render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path=':orgSlug/settings/*' element={<Probe />} />
        <Route path='*' element={<Probe />} />
      </Routes>
    </MemoryRouter>
  );
  return screen.getByTestId("probe").textContent;
};

afterEach(cleanup);

describe("useSettingsRoute", () => {
  it("reads the section and tab from the address", () => {
    expect(at("/acme/settings/organization/members")).toBe("true|organization.members|-");
    expect(at("/acme/settings/organization/api_tokens/policy")).toBe(
      "true|organization.api_tokens|policy"
    );
  });

  it("reports the index as in-settings with no section", () => {
    expect(at("/acme/settings")).toBe("true|-|-");
  });

  it("works for an org whose slug starts with the word settings", () => {
    // It used to search the pathname for the first "/settings", which matched the *org*
    // segment here: `/settings-co/settings/organization/members` parsed as
    // `-co.settings.organization.members`, resolved to nothing, and left every settings page
    // in that org blank on desktop and sectionless on a phone. Nothing reserves the slug.
    expect(at("/settings-co/settings/organization/members")).toBe("true|organization.members|-");
    expect(at("/settings/settings/organization/crew")).toBe("true|organization.crew|-");
    expect(at("/settingsinc/settings/workspace/oltp")).toBe("true|workspace.oltp|-");
  });

  it("is not in settings on another page of such an org", () => {
    expect(at("/settings-co/home")).toBe("false|-|-");
  });
});
