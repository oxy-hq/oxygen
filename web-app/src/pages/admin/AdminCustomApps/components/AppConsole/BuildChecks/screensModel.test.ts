import { AxiosError, type AxiosResponse } from "axios";
import { describe, expect, it } from "vitest";
import type { BuildCheckDetail } from "@/types/appBuildChecks";
import {
  awaitsALook,
  changedScreens,
  changesCount,
  newErrors,
  ratioText,
  rollbackOf,
  screenName,
  screensLine,
  seenOf,
  seenRefusal,
  smokeScreens,
  unlistedScreens
} from "./screensModel";

const viewport = (name: string, over: Record<string, unknown> = {}) => ({
  name,
  width: name === "desk" ? 1440 : 390,
  height: 900,
  ready: true,
  ready_ms: 900,
  environment: "staging",
  build_id: "9087d7b",
  page_errors: [],
  failed_requests: [],
  refused_writes: [],
  ...over
});
const screen = (path: string, desk = {}, phone = {}) => ({
  path,
  viewports: [viewport("desk", desk), viewport("phone", phone)]
});

const refused = (status: number, code: string) =>
  new AxiosError("refused", "ERR", undefined, undefined, {
    status,
    data: { code, message: "the server's own words" }
  } as AxiosResponse);

describe("smokeScreens", () => {
  it("reads one line per screen, the two viewports each ok or not", () => {
    const [root] = smokeScreens({ screens: [screen("")] });
    expect(root.name).toBe("/");
    expect(root.failing).toBe(false);
    expect(root.viewports).toEqual([
      { name: "desk", ready: true, ok: true, text: "ready in 900ms" },
      { name: "phone", ready: true, ok: true, text: "ready in 900ms" }
    ]);
  });

  it("puts the failing screens first, in the runner's order otherwise", () => {
    const detail = {
      screens: [
        screen(""),
        screen("stores", {}, { ready: false, ready_ms: null }),
        screen("flags"),
        screen("reorder/plans", { page_errors: ["TypeError: x is undefined"] })
      ]
    };
    expect(smokeScreens(detail).map((s) => [s.name, s.failing])).toEqual([
      ["/stores", true],
      ["/reorder/plans", true],
      ["/", false],
      ["/flags", false]
    ]);
    expect(smokeScreens(detail)[0].viewports[1]).toEqual({
      name: "phone",
      ready: false,
      ok: false,
      text: "never ready"
    });
    // Ready, but it threw: not ok, and not "never ready" either.
    expect(smokeScreens(detail)[1].viewports[0]).toMatchObject({ ready: true, ok: false });
  });

  it("keeps an error once, with every viewport that threw it", () => {
    const boom = { page_errors: ["TypeError: x is undefined"] };
    const [stores] = smokeScreens({
      screens: [screen("stores", boom, { page_errors: [...boom.page_errors, "second"] })]
    });
    expect(stores.errors).toEqual([
      { viewports: ["desk", "phone"], value: "TypeError: x is undefined" },
      { viewports: ["phone"], value: "second" }
    ]);
  });

  it("warns about a failed request or a refused write without failing the screen", () => {
    const call = { url: "https://acme/fn/list-plans", status: 403, error: null };
    const write = "POST /fn/save-plan";
    const [plans] = smokeScreens({
      screens: [
        screen(
          "plans",
          { failed_requests: [call], refused_writes: [write, {}] },
          { refused_writes: [write] }
        )
      ]
    });
    expect(plans.failing).toBe(false);
    expect(plans.failedRequests).toEqual([{ viewports: ["desk"], value: call }]);
    // Counted whatever an entry holds; named once when the runner named it.
    expect(plans.refusedWrites).toBe(3);
    expect(plans.refused).toEqual([write]);
  });

  it("reads a detail with none of the fields, or the wrong ones, as no screens", () => {
    const shapes: unknown[] = [
      null,
      undefined,
      {},
      { findings: [] },
      { screens: null },
      { screens: "14" },
      { screens: [null, 3, { viewports: [] }, { path: 7 }] }
    ];
    for (const detail of shapes) {
      expect(smokeScreens(detail as BuildCheckDetail)).toEqual([]);
      expect(changedScreens(detail as BuildCheckDetail)).toEqual([]);
      expect(newErrors(detail as BuildCheckDetail)).toEqual([]);
      expect(seenOf(detail as BuildCheckDetail)).toBeNull();
    }
    // A screen whose viewports are missing still reads, as a screen with none.
    expect(smokeScreens({ screens: [{ path: "stores" }] })).toMatchObject([
      { name: "/stores", viewports: [], failing: false }
    ]);
  });
});

describe("screensLine", () => {
  it("counts the screens, says when the runner found them, and how many it left out", () => {
    expect(screensLine({ discovered: false, screens_omitted: 0 }, 14)).toBe("14 screens");
    expect(screensLine({ discovered: true, screens_omitted: 3 }, 30)).toBe(
      "30 screens · discovered · 3 left out"
    );
    expect(screensLine({}, 1)).toBe("1 screen");
    expect(screensLine({}, 0)).toBe("");
  });
});

describe("unlistedScreens", () => {
  it("names the linked screens no check opened, and counts them all", () => {
    expect(
      unlistedScreens({ screens_unlisted: ["plans", "stores/12", 7], screens_unlisted_total: 61 })
    ).toEqual({ names: ["/plans", "/stores/12"], total: 61 });
    // A runner that sent the names and no count: the names are the count.
    expect(unlistedScreens({ screens_unlisted: ["plans"] })?.total).toBe(1);
  });

  it("is null when the detail names none, whatever else it holds", () => {
    expect(unlistedScreens({ discovered: true, screens_omitted: 2 })).toBeNull();
    expect(unlistedScreens({ screens_unlisted: [], screens_unlisted_total: 0 })).toBeNull();
    expect(unlistedScreens(null)).toBeNull();
  });
});

describe("rollbackOf", () => {
  it("reads what the platform did, naming the build as it was published", () => {
    expect(
      rollbackOf({
        rollback: { rolled_back: true, to: { id: "row-o", build_id: "0ddba11" }, at: "t" }
      })
    ).toEqual({ rolledBack: true, to: "0ddba11", reason: null, at: "t" });
    expect(rollbackOf({ rollback: { rolled_back: true, to: { id: "row-o" } } })?.to).toBe("row-o");
    expect(
      rollbackOf({ rollback: { rolled_back: false, reason: "no other build", at: "t" } })
    ).toEqual({ rolledBack: false, to: null, reason: "no other build", at: "t" });
  });

  it("is null for a detail that records none, or one it cannot read", () => {
    for (const detail of [{}, { rollback: null }, { rollback: "yes" }, { rollback: { to: {} } }]) {
      expect(rollbackOf(detail)).toBeNull();
    }
    expect(rollbackOf(null)).toBeNull();
  });
});

describe("newErrors", () => {
  it("keeps each new error with its screen and viewport", () => {
    const detail = {
      new_errors: [
        { path: "stores", viewport: "desk", error: "TypeError: x" },
        { path: "stores", viewport: "phone" },
        { viewport: "desk", error: "no path" }
      ]
    };
    expect(newErrors(detail)).toEqual([
      { path: "stores", viewport: "desk", error: "TypeError: x" }
    ]);
  });
});

describe("changedScreens", () => {
  const shot = (path: string, viewport: string, over: Record<string, unknown> = {}) => ({
    path,
    viewport,
    changed: false,
    ratio: 0,
    staging: `shot-staging-${path || "root"}-${viewport}.png`,
    production: `shot-production-${path || "root"}-${viewport}.png`,
    diff: null,
    production_ready: true,
    production_errors: [],
    ...over
  });

  it("puts the changed screens first, in the runner's order otherwise", () => {
    const detail = {
      compared: 4,
      changed: 2,
      screens: [
        shot("", "desk"),
        shot("stores", "desk", { changed: true, ratio: 0.031, diff: "diff-stores-desk.png" }),
        shot("", "phone"),
        shot("stores", "phone", { changed: true, ratio: 0.2 })
      ]
    };
    const screens = changedScreens(detail);
    expect(screens.map((s) => s.key)).toEqual(["stores@desk", "stores@phone", "@desk", "@phone"]);
    expect(screens[0]).toMatchObject({ name: "/stores", diff: "diff-stores-desk.png" });
    expect(changesCount(detail, screens)).toEqual({ changed: 2, compared: 4 });
  });

  it("counts from its rows when the detail carries no counts", () => {
    const screens = changedScreens({
      screens: [shot("a", "desk", { changed: true }), shot("b", "desk")]
    });
    expect(changesCount({}, screens)).toEqual({ changed: 1, compared: 2 });
  });

  it("reads a screen with no pictures named as having none", () => {
    const [bare] = changedScreens({ screens: [{ path: "stores", changed: true }] });
    expect(bare).toMatchObject({
      viewport: "",
      staging: null,
      production: null,
      diff: null,
      ratio: null,
      production_ready: null,
      production_errors: []
    });
  });
});

describe("ratioText", () => {
  it("says the share that differs, and nothing for a screen that did not change", () => {
    expect(ratioText({ changed: true, ratio: 0.031 })).toBe("3.1%");
    expect(ratioText({ changed: true, ratio: 0.2 })).toBe("20%");
    expect(ratioText({ changed: true, ratio: 0.0004 })).toBe("<0.1%");
    // A different size, with no share measured.
    expect(ratioText({ changed: true, ratio: null })).toBe("differs");
    expect(ratioText({ changed: false, ratio: 0 })).toBe("");
  });
});

describe("seenOf and seenRefusal", () => {
  it("names who saw the changes only when someone did", () => {
    expect(seenOf({ seen: { by: "mars@oxy.tech", at: "2026-11-22T10:00:00Z" } })).toEqual({
      by: "mars@oxy.tech",
      at: "2026-11-22T10:00:00Z"
    });
    expect(seenOf({ seen: null })).toBeNull();
    expect(seenOf({ seen: { at: "2026-11-22T10:00:00Z" } as never })).toBeNull();
  });

  it("says a 409 nothing_to_see plainly, and anything else in the server's words", () => {
    expect(seenRefusal(refused(409, "nothing_to_see"))).toBe("Nothing changed.");
    expect(seenRefusal(refused(500, "boom"))).toBe("the server's own words");
    expect(seenRefusal(new Error(""))).toBe("Couldn't mark it seen.");
  });
});

describe("awaitsALook", () => {
  const changes = (detail: object) => ({ kind: "changes", detail: detail as BuildCheckDetail });
  const differs = { changed: 2, compared: 4, seen: null };

  it("is true only for changed screens nobody has seen, on an app that asks for it", () => {
    expect(awaitsALook(changes(differs), true)).toBe(true);
    expect(awaitsALook(changes(differs), false)).toBe(false);
    expect(awaitsALook(changes({ ...differs, changed: 0 }), true)).toBe(false);
    const seen = { ...differs, seen: { by: "ops@example.test", at: "2026-11-22T10:00:00Z" } };
    expect(awaitsALook(changes(seen), true)).toBe(false);
    expect(awaitsALook({ kind: "smoke", detail: differs as BuildCheckDetail }, true)).toBe(false);
    expect(awaitsALook({ kind: "changes", detail: null }, true)).toBe(false);
  });
});

describe("screenName", () => {
  it("names the root and a path alike", () => {
    expect(screenName("")).toBe("/");
    expect(screenName("stores")).toBe("/stores");
    expect(screenName("/reorder/plans")).toBe("/reorder/plans");
  });
});
