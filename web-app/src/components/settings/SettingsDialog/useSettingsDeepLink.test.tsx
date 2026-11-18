// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import useSettingsDialog from "@/stores/useSettingsDialog";
import { targetFromParam, useSettingsDeepLink } from "./useSettingsDeepLink";

describe("targetFromParam", () => {
  it("accepts every section the nav has, organization and workspace alike", () => {
    expect(targetFromParam("organization.general")).toEqual({ section: "organization.general" });
    expect(targetFromParam("organization.crew")).toEqual({ section: "organization.crew" });
    expect(targetFromParam("organization.api_tokens")).toEqual({
      section: "organization.api_tokens"
    });
    expect(targetFromParam("workspace.databases")).toEqual({ section: "workspace.databases" });
    expect(targetFromParam("preferences.appearance")).toEqual({
      section: "preferences.appearance"
    });
  });

  it("opens a tab named as a third segment", () => {
    expect(targetFromParam("organization.api_tokens.personal")).toEqual({
      section: "organization.api_tokens",
      tab: "personal"
    });
    expect(targetFromParam("organization.api_tokens.service_accounts")).toEqual({
      section: "organization.api_tokens",
      tab: "service_accounts"
    });
    expect(targetFromParam("organization.secrets.app")).toEqual({
      section: "organization.secrets",
      tab: "app"
    });
  });

  // The links already out there: token emails, published oxyc versions, bookmarks.
  it("sends the old token and secret pages to where they live now", () => {
    expect(targetFromParam("account.tokens")).toEqual({
      section: "organization.api_tokens",
      tab: "personal"
    });
    expect(targetFromParam("organization.api_access")).toEqual({
      section: "organization.api_tokens",
      tab: "service_accounts"
    });
    expect(targetFromParam("workspace.api_keys")).toEqual({
      section: "organization.api_tokens",
      tab: "all"
    });
    expect(targetFromParam("workspace.secrets")).toEqual({ section: "organization.secrets" });
  });

  it("drops anything the dialog does not have, and the empty cases", () => {
    // A typo must not open some other section.
    expect(targetFromParam("organization.crews")).toBeNull();
    expect(targetFromParam("admin.billing")).toBeNull();
    // A tab the section does not have, or a tab on a section with none.
    expect(targetFromParam("organization.api_tokens.billing")).toBeNull();
    expect(targetFromParam("organization.crew.workers")).toBeNull();
    // An old id with a tab glued on is not an old id.
    expect(targetFromParam("account.tokens.personal")).toBeNull();
    expect(targetFromParam("")).toBeNull();
    expect(targetFromParam(null)).toBeNull();
    expect(targetFromParam(undefined)).toBeNull();
  });
});

/** The query string the hook left behind, read by the component that runs it. */
let search = "";

const Probe = () => {
  useSettingsDeepLink();
  search = useLocation().search;
  return null;
};

describe("useSettingsDeepLink", () => {
  const visit = (url: string) =>
    render(
      <MemoryRouter initialEntries={[url]}>
        <Probe />
      </MemoryRouter>
    );

  beforeEach(() => {
    useSettingsDialog.setState({ isOpen: false, section: "organization.general", tab: null });
  });
  afterEach(cleanup);

  it("opens an old link at the page that replaced it, on the matching tab, then strips it", () => {
    visit("/acme/home?settings=account.tokens&tab=keep");
    expect(useSettingsDialog.getState()).toMatchObject({
      isOpen: true,
      section: "organization.api_tokens",
      tab: "personal"
    });
    // Only its own param goes, so a refresh does not reopen the dialog.
    expect(search).toBe("?tab=keep");
  });

  it("opens Organization → Secrets from the old Workspace → Secrets link", () => {
    visit("/acme/home?settings=workspace.secrets");
    expect(useSettingsDialog.getState()).toMatchObject({
      isOpen: true,
      section: "organization.secrets",
      tab: null
    });
  });

  it("leaves the dialog shut for an id it does not know, and still strips it", () => {
    visit("/acme/home?settings=workspace.nonsense");
    expect(useSettingsDialog.getState().isOpen).toBe(false);
    expect(search).toBe("");
  });
});

describe("the dialog store", () => {
  beforeEach(() => {
    useSettingsDialog.setState({ isOpen: false, section: "organization.general", tab: null });
  });

  it("drops the tab when another section opens, and keeps it when the same one does", () => {
    const { open, setTab } = useSettingsDialog.getState();
    open("organization.api_tokens", "policy");
    open("organization.api_tokens");
    expect(useSettingsDialog.getState().tab).toBe("policy");
    open("organization.members");
    expect(useSettingsDialog.getState().tab).toBeNull();
    open("organization.api_tokens");
    setTab("all");
    expect(useSettingsDialog.getState()).toMatchObject({
      section: "organization.api_tokens",
      tab: "all"
    });
  });
});
