/**
 * Which screens of a build the promote checks open: the rules of
 * `screens.ts`, over plain strings. No page is opened here.
 */

import { describe, expect, it } from "vitest";
import {
  declaredScreens,
  diffName,
  discoveredScreens,
  screenLabel,
  screenPath,
  screenSlugs,
  screenUrl,
  shotName,
  textName,
  unlistedScreens
} from "./screens.js";

const ORIGIN = "https://app.example.com";
const BASE = `${ORIGIN}/customer-apps/acme/store/`;
const APP = { url: BASE, origin: ORIGIN, basePath: "/customer-apps/acme/store/" };

describe("discoveredScreens", () => {
  it("puts the root first, then each same-app link once, in document order", () => {
    const list = discoveredScreens(
      [`${BASE}stores`, `${BASE}reorder/plans`, BASE, `${BASE}stores`, `${BASE}settings`],
      APP,
      30
    );
    expect(list).toEqual({
      paths: ["", "stores", "reorder/plans", "settings"],
      discovered: true,
      omitted: 0
    });
  });

  it("is the root alone for a page with no links", () => {
    expect(discoveredScreens([], APP, 30)).toEqual({ paths: [""], discovered: true, omitted: 0 });
  });

  it("drops the query and the hash, and a trailing slash, before telling screens apart", () => {
    const list = discoveredScreens(
      [
        `${BASE}stores?sort=name`,
        `${BASE}stores#top`,
        `${BASE}stores/`,
        `${BASE}#/hash-route`,
        // The base without its slash is the root too.
        `${ORIGIN}/customer-apps/acme/store`
      ],
      APP,
      30
    );
    expect(list.paths).toEqual(["", "stores"]);
  });

  it("ignores every link that is not a screen of this app", () => {
    const list = discoveredScreens(
      [
        "https://elsewhere.example.com/customer-apps/acme/store/stores",
        `${ORIGIN}/customer-apps/acme/other-app/`,
        `${ORIGIN}/api/orgs`,
        `${BASE}../other-app/x`,
        "mailto:someone@example.com",
        "javascript:void(0)",
        `${BASE}kept`
      ],
      APP,
      30
    );
    expect(list.paths).toEqual(["", "kept"]);
  });

  it("never lists a function route, the runtime's endpoints, or a link to a file", () => {
    const list = discoveredScreens(
      [
        `${BASE}fn/save`,
        `${BASE}fn`,
        `${BASE}__oxy/beacon`,
        `${BASE}assets/index-abc.js`,
        `${BASE}files/report.pdf`,
        `${BASE}export.csv`,
        // A page that happens to be written as a file is still a page.
        `${BASE}help.html`,
        // Not the reserved word, only a path that starts like it.
        `${BASE}fnords`
      ],
      APP,
      30
    );
    expect(list.paths).toEqual(["", "help.html", "fnords"]);
  });

  it("stops at the cap and says how many it left out", () => {
    const hrefs = Array.from({ length: 40 }, (_, i) => `${BASE}s${i}`);
    const list = discoveredScreens(hrefs, APP, 30);
    expect(list.paths).toHaveLength(30);
    expect(list.paths.slice(0, 3)).toEqual(["", "s0", "s1"]);
    // Forty links and the root, thirty kept.
    expect(list.omitted).toBe(11);
  });
});

describe("unlistedScreens", () => {
  it("names each screen of the app that is linked and not listed, once, in the order read", () => {
    const unlisted = unlistedScreens(
      [
        `${BASE}stores`,
        `${BASE}plans?week=47`,
        `${BASE}stores/12/`,
        `${BASE}plans#top`,
        BASE,
        "https://elsewhere.example.com/pricing",
        `${ORIGIN}/customer-apps/acme/other/`,
        `${BASE}fn/save`,
        `${BASE}export.csv`,
        `${BASE}settings`
      ],
      APP,
      ["", "stores"]
    );
    expect(unlisted).toEqual({ paths: ["plans", "stores/12", "settings"], total: 3 });
  });

  it("compares a link by its path alone, against a list that declares queries", () => {
    // The app picks its screen by `?module=`: every such link is the root's, which is listed.
    const unlisted = unlistedScreens(
      [`${BASE}?module=temps`, `${BASE}?module=waste`, `${BASE}guide`],
      APP,
      ["?module=waste"]
    );
    expect(unlisted).toEqual({ paths: ["guide"], total: 1 });
  });

  it("is empty when every linked screen is listed", () => {
    expect(unlistedScreens([`${BASE}stores`, BASE], APP, ["", "stores"])).toEqual({
      paths: [],
      total: 0
    });
  });

  it("names the first of them up to the cap, and still counts them all", () => {
    const hrefs = Array.from({ length: 70 }, (_, i) => `${BASE}stores/${i}`);
    const unlisted = unlistedScreens(hrefs, APP, [""], 50);
    expect(unlisted.paths).toHaveLength(50);
    expect(unlisted.paths.slice(0, 2)).toEqual(["stores/0", "stores/1"]);
    expect(unlisted.total).toBe(70);
  });

  it("leaves out a path too long to declare", () => {
    const long = `${BASE}${"a".repeat(257)}`;
    expect(unlistedScreens([long, `${BASE}${"b".repeat(256)}`], APP, [""]).total).toBe(1);
  });
});

describe("declaredScreens", () => {
  it("is undefined when the build declares none: they are discovered then", () => {
    expect(declaredScreens(null, APP, 30)).toBeUndefined();
    expect(declaredScreens(undefined, APP, 30)).toBeUndefined();
    expect(declaredScreens("stores", APP, 30)).toBeUndefined();
  });

  it("keeps the build's own order, each path once", () => {
    expect(declaredScreens(["stores", "", "reorder/plans", "stores"], APP, 30)).toEqual({
      paths: ["stores", "", "reorder/plans"],
      discovered: false,
      omitted: 0
    });
  });

  it("reads each entry as app-relative, and drops one that leaves the app or names a function", () => {
    const list = declaredScreens(
      ["/stores", "../other-app/", "https://elsewhere.example.com/", "fn/save", 7, "ok?tab=1"],
      APP,
      30
    );
    expect(list?.paths).toEqual(["stores", "ok?tab=1"]);
  });

  it("keeps a declared screen's query as it was written, and never its hash", () => {
    const list = declaredScreens(
      [
        "",
        "?module=waste",
        "?module=temps&view=week",
        "stores?tab=open#top",
        "stores?",
        "fn/x?a=1"
      ],
      APP,
      30
    );
    expect(list?.paths).toEqual([
      "",
      "?module=waste",
      "?module=temps&view=week",
      "stores?tab=open",
      "stores"
    ]);
    // Each is opened at the app's own base, its query after it.
    expect(screenUrl(APP, "?module=waste")).toBe(`${BASE}?module=waste`);
    expect(screenUrl(APP, "stores?tab=open")).toBe(`${BASE}stores?tab=open`);
    expect(screenLabel("?module=waste")).toBe("/?module=waste");
  });

  it("is the root alone when nothing declared is a screen of this app", () => {
    expect(declaredScreens([], APP, 30)?.paths).toEqual([""]);
    expect(declaredScreens(["fn/x"], APP, 30)?.paths).toEqual([""]);
  });

  it("stops at the cap too", () => {
    const list = declaredScreens(
      Array.from({ length: 5 }, (_, i) => `s${i}`),
      APP,
      2
    );
    expect(list).toEqual({ paths: ["s0", "s1"], discovered: false, omitted: 3 });
  });
});

describe("screen names", () => {
  it("opens a screen at the app's base plus its path, and labels it as a person reads it", () => {
    expect(screenUrl(APP, "")).toBe(BASE);
    expect(screenUrl(APP, "reorder/plans")).toBe(`${BASE}reorder/plans`);
    expect(screenLabel("")).toBe("/");
    expect(screenLabel("stores")).toBe("/stores");
    expect(screenPath(`${BASE}stores`, APP)).toBe("stores");
  });

  it("slugs a path with `/` as `--` and the root as `root`", () => {
    const slugs = screenSlugs(["", "stores", "reorder/plans"]);
    expect(Object.fromEntries(slugs)).toEqual({
      "": "root",
      stores: "stores",
      "reorder/plans": "reorder--plans"
    });
    expect(shotName("staging", "stores", "desk")).toBe("shot-staging-stores-desk.png");
    expect(shotName("production", "reorder--plans", "phone")).toBe(
      "shot-production-reorder--plans-phone.png"
    );
    expect(diffName("stores", "desk")).toBe("diff-stores-desk.png");
    expect(textName("staging", "root", "phone")).toBe("text-staging-root-phone.json");
  });

  it("slugs a screen picked by a query by the query's own words", () => {
    const slugs = screenSlugs(["?module=waste", "?module=temps&view=week", "stores?tab=open", "?"]);
    expect(Object.fromEntries(slugs)).toEqual({
      "?module=waste": "module-waste",
      "?module=temps&view=week": "module-temps-view-week",
      "stores?tab=open": "stores-tab-open",
      // Nothing left to name it by.
      "?": "screen"
    });
    for (const slug of slugs.values()) {
      expect(shotName("production", slug, "phone")).toMatch(/^[a-z0-9._-]{1,120}$/);
    }
  });

  it("keeps every file name inside what the platform takes, and no two screens on one name", () => {
    const long = `${"a".repeat(200)}/b`;
    const slugs = screenSlugs(["Stores", "stores", "caf%C3%A9 menu", "root", "", long]);
    const all = Array.from(slugs.values());
    expect(new Set(all).size).toBe(all.length);
    expect(slugs.get("Stores")).toBe("stores");
    expect(slugs.get("stores")).toBe("stores-2");
    expect(slugs.get("caf%C3%A9 menu")).toBe("caf-c3-a9-menu");
    // The root's slug was taken by a screen called `root`.
    expect(slugs.get("")).toBe("root-2");
    for (const slug of all) {
      for (const name of [
        shotName("production", slug, "phone"),
        textName("production", slug, "phone")
      ]) {
        expect(name).toMatch(/^[a-z0-9._-]{1,120}$/);
      }
    }
  });
});
