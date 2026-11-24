import { describe, expect, it } from "vitest";
import {
  settingsHref,
  settingsPathFor,
  settingsRootHref,
  targetFromId,
  targetFromPath
} from "./settingsRoute";

/**
 * Section ids are a contract: `?settings=` links live in notification emails the backend has
 * already sent and in URLs published `oxyc` versions print. The route spells the same ids with
 * slashes, so these assert that the two spellings stay one thing.
 */

describe("targetFromId", () => {
  it("accepts every section the nav has", () => {
    expect(targetFromId("organization.general")).toEqual({ section: "organization.general" });
    expect(targetFromId("organization.crew")).toEqual({ section: "organization.crew" });
    expect(targetFromId("workspace.databases")).toEqual({ section: "workspace.databases" });
    expect(targetFromId("preferences.appearance")).toEqual({ section: "preferences.appearance" });
  });

  it("opens a tab named as a third segment", () => {
    expect(targetFromId("organization.api_tokens.personal")).toEqual({
      section: "organization.api_tokens",
      tab: "personal"
    });
    expect(targetFromId("organization.secrets.app")).toEqual({
      section: "organization.secrets",
      tab: "app"
    });
  });

  it("sends the old token and secret ids to where they live now", () => {
    expect(targetFromId("account.tokens")).toEqual({
      section: "organization.api_tokens",
      tab: "personal"
    });
    expect(targetFromId("organization.api_access")).toEqual({
      section: "organization.api_tokens",
      tab: "service_accounts"
    });
    expect(targetFromId("workspace.api_keys")).toEqual({
      section: "organization.api_tokens",
      tab: "all"
    });
    expect(targetFromId("workspace.secrets")).toEqual({ section: "organization.secrets" });
  });

  it("refuses a key that only exists on Object.prototype", () => {
    // `RENAMED_SECTIONS` is an object literal, so a bare index on it answers truthily for
    // these. A crafted `?settings=constructor` then carried a function around as if it were
    // a settings target.
    for (const crafted of ["constructor", "toString", "__proto__", "valueOf", "hasOwnProperty"]) {
      expect(targetFromId(crafted), crafted).toBeNull();
      expect(targetFromPath(crafted), crafted).toBeNull();
    }
  });

  it("drops anything the surface does not have", () => {
    expect(targetFromId("organization.crews")).toBeNull();
    expect(targetFromId("admin.billing")).toBeNull();
    expect(targetFromId("organization.api_tokens.billing")).toBeNull();
    expect(targetFromId("organization.crew.workers")).toBeNull();
    expect(targetFromId("account.tokens.personal")).toBeNull();
    expect(targetFromId("")).toBeNull();
    expect(targetFromId(null)).toBeNull();
    expect(targetFromId(undefined)).toBeNull();
  });
});

describe("targetFromPath", () => {
  it("reads the same ids with slashes for dots", () => {
    expect(targetFromPath("organization/members")).toEqual({ section: "organization.members" });
    expect(targetFromPath("/organization/members/")).toEqual({ section: "organization.members" });
    expect(targetFromPath("organization/api_tokens/personal")).toEqual({
      section: "organization.api_tokens",
      tab: "personal"
    });
  });

  it("treats an empty path as the index, not as a section", () => {
    expect(targetFromPath("")).toBeNull();
    expect(targetFromPath("/")).toBeNull();
    expect(targetFromPath(undefined)).toBeNull();
  });

  it("redirects an old id spelled as a path, as an email's link would arrive", () => {
    expect(targetFromPath("account/tokens")).toEqual({
      section: "organization.api_tokens",
      tab: "personal"
    });
  });

  it("refuses a path naming no section", () => {
    expect(targetFromPath("organization/nonsense")).toBeNull();
    expect(targetFromPath("nonsense")).toBeNull();
  });
});

describe("building an address", () => {
  it("round-trips every id through its path", () => {
    for (const id of [
      "organization.general",
      "organization.api_tokens",
      "workspace.oltp",
      "account.notifications",
      "preferences.appearance"
    ] as const) {
      const path = settingsPathFor(id).replace(/^settings\//, "");
      expect(targetFromPath(path), id).toEqual({ section: id });
    }
  });

  it("round-trips a tab too", () => {
    const path = settingsPathFor("organization.api_tokens", "policy").replace(/^settings\//, "");
    expect(targetFromPath(path)).toEqual({ section: "organization.api_tokens", tab: "policy" });
  });

  it("puts the org in front, and nothing in local mode, which has no org segment", () => {
    expect(settingsHref("acme", "organization.members")).toBe(
      "/acme/settings/organization/members"
    );
    expect(settingsHref("acme", "organization.api_tokens", "all")).toBe(
      "/acme/settings/organization/api_tokens/all"
    );
    expect(settingsHref(undefined, "workspace.oltp")).toBe("/settings/workspace/oltp");
    expect(settingsRootHref("acme")).toBe("/acme/settings");
    expect(settingsRootHref(undefined)).toBe("/settings");
  });
});
