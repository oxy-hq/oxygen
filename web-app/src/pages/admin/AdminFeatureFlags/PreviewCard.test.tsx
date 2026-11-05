// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const orgList = vi.hoisted(() => ({
  value: {
    orgs: [] as { id: string; name: string }[],
    isLoading: false,
    isIncomplete: false,
    refetch: () => {}
  }
}));
vi.mock("@/hooks/api/featureFlags", () => ({
  useAdminOrgOptions: () => orgList.value,
  useFeatureFlagEvaluation: () => ({ data: undefined, isError: false, isPending: true })
}));

import { PreviewCard } from "./PreviewCard";

const typeOrg = (name: string) =>
  fireEvent.change(screen.getByTestId("feature-flag-preview-org"), { target: { value: name } });
const hint = () => screen.getByTestId("feature-flag-preview-hint").textContent;

describe("PreviewCard", () => {
  afterEach(cleanup);
  beforeEach(() => {
    orgList.value = { orgs: [], isLoading: false, isIncomplete: false, refetch: vi.fn() };
  });

  it("calls a name absent once every org has loaded", () => {
    render(<PreviewCard />);
    typeOrg("Acme");
    expect(hint()).toBe("No org named “Acme”.");
  });

  it("waits while the org list is still arriving", () => {
    orgList.value.isLoading = true;
    render(<PreviewCard />);
    typeOrg("Acme");
    expect(hint()).toBe("Loading orgs…");
  });

  it("says it couldn't check when a page of orgs failed, and offers a retry", () => {
    orgList.value.isIncomplete = true;
    render(<PreviewCard />);
    typeOrg("Acme");
    expect(hint()).toMatch(/Couldn't load every org, so “Acme” may still exist/);
    fireEvent.click(screen.getByTestId("feature-flag-preview-orgs-retry"));
    expect(orgList.value.refetch).toHaveBeenCalledOnce();
  });

  it("asks for the uuid when two orgs share the name", () => {
    orgList.value.orgs = [
      { id: "a", name: "Dupe" },
      { id: "b", name: "dupe" }
    ];
    render(<PreviewCard />);
    typeOrg("Dupe");
    expect(hint()).toBe("More than one org is named “Dupe”. Paste its uuid.");
  });
});
