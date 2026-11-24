// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LegacySettingsParamRedirect } from "./LegacySettingsParamRedirect";

vi.mock("@/stores/useCurrentOrg", () => ({
  default: (selector: (s: { org: { slug: string } }) => unknown) =>
    selector({ org: { slug: "acme" } })
}));

/**
 * `?settings=` is not a legacy of the web app alone: the Rust backend writes it into
 * notification emails already sent, and published `oxyc` versions print it. Those links cannot
 * be reissued, so this redirect is permanent and these are the cases it has to keep answering.
 */

const Here = () => {
  const { pathname, search } = useLocation();
  return <output data-testid='here'>{pathname + search}</output>;
};

const land = (url: string) =>
  render(
    <MemoryRouter initialEntries={[url]}>
      <LegacySettingsParamRedirect />
      <Routes>
        <Route path='*' element={<Here />} />
      </Routes>
    </MemoryRouter>
  );

const at = () => screen.getByTestId("here").textContent;

afterEach(cleanup);

describe("LegacySettingsParamRedirect", () => {
  it("sends a section link to its page", () => {
    land("/acme/home?settings=organization.members");
    expect(at()).toBe("/acme/settings/organization/members");
  });

  it("sends a tab link to that tab — the shape the token emails use", () => {
    land("/acme/home?settings=organization.api_tokens.personal");
    expect(at()).toBe("/acme/settings/organization/api_tokens/personal");
  });

  it("sends an id that moved to where it lives now", () => {
    land("/acme/home?settings=account.tokens");
    expect(at()).toBe("/acme/settings/organization/api_tokens/personal");
    cleanup();
    land("/acme/home?settings=workspace.secrets");
    expect(at()).toBe("/acme/settings/organization/secrets");
  });

  it("keeps the other parameters, which the Slack return rides on", () => {
    land("/acme/home?slack_installed=ok&settings=organization.integration");
    expect(at()).toBe("/acme/settings/organization/integration?slack_installed=ok");
  });

  it("drops an id naming no page, and goes nowhere", () => {
    land("/acme/home?settings=workspace.nonsense");
    expect(at()).toBe("/acme/home");
  });

  it("leaves an address with no parameter alone", () => {
    land("/acme/home?tab=keep");
    expect(at()).toBe("/acme/home?tab=keep");
  });
});
