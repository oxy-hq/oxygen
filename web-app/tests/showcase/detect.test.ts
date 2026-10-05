import { describe, expect, it } from "vitest";
import { commitType, detect, isUiSource, showcaseHint } from "./detect";

const base = {
  title: "feat: kiosk exit",
  body: "",
  labels: [],
  files: ["web-app/src/pages/kiosk/Exit.tsx"]
};

describe("commitType", () => {
  it("reads the type the announcement groups by", () => {
    expect(commitType("feat: a thing (#1)")).toBe("feat");
    expect(commitType("Fix(web): a thing")).toBe("fix");
    expect(commitType("feat!: breaking")).toBe("feat");
    expect(commitType("no type here")).toBeUndefined();
  });
});

describe("isUiSource", () => {
  it("keeps rendered source and drops tests, stories and declarations", () => {
    expect(isUiSource("web-app/src/pages/a/index.tsx")).toBe(true);
    expect(isUiSource("web-app/src/styles/x.css")).toBe(true);
    expect(isUiSource("web-app/src/pages/a/index.test.tsx")).toBe(false);
    expect(isUiSource("web-app/src/pages/__tests__/a.ts")).toBe(false);
    expect(isUiSource("web-app/src/types/x.d.ts")).toBe(false);
    expect(isUiSource("web-app/tests/agentic/runner/cli.ts")).toBe(false);
    expect(isUiSource("crates/app/src/main.rs")).toBe(false);
  });
});

describe("showcaseHint", () => {
  it("returns the section up to the next heading", () => {
    const body =
      "## Summary\nstuff\n\n## Showcase\nOpen the kiosk as owner.\nTap Exit.\n\n## Test plan\n- x";
    expect(showcaseHint(body)).toBe("Open the kiosk as owner.\nTap Exit.");
  });
  it("is undefined when absent or empty", () => {
    expect(showcaseHint("## Summary\nx")).toBeUndefined();
    expect(showcaseHint("## Showcase\n\n## Next")).toBeUndefined();
  });
});

describe("detect", () => {
  it("takes a feat that changes a screen", () => {
    expect(detect(base)).toEqual({ candidate: true, type: "feat" });
  });
  it("drops internal types, labelled PRs and changes with no screen", () => {
    expect(detect({ ...base, title: "chore: bump" }).candidate).toBe(false);
    expect(detect({ ...base, labels: ["no-showcase"] }).candidate).toBe(false);
    expect(detect({ ...base, files: ["crates/app/src/lib.rs"] })).toEqual({
      candidate: false,
      reason: "no web-app screen changed"
    });
  });
  it("lets an author's steer override the path rule", () => {
    const d = detect({
      ...base,
      files: ["crates/app/src/lib.rs"],
      body: "## Showcase\nThe error banner."
    });
    expect(d).toEqual({ candidate: true, type: "feat", hint: "The error banner." });
  });
});
