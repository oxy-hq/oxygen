import { afterEach, describe, expect, it, vi } from "vitest";
import { stubFetch } from "../testing/stub-fetch.js";
import { CliError } from "../util/errors.js";
import { uploadBundle } from "./server.js";
import { formatMs, formatStages, PUBLISH_STAGE_HEADER, refusalHeadline } from "./stages.js";

const TARGET = "https://oxy.example.com";

afterEach(() => vi.unstubAllGlobals());

const upload = (token = "oxy_pat_person") =>
  uploadBundle({ target: TARGET, token, fields: [], tarball: Buffer.from("x") });

describe("formatMs", () => {
  it("prints ms under a second, then seconds", () => {
    expect([0, 12.4, 999, 1000, 1250, 42_000].map(formatMs)).toEqual([
      "0ms",
      "12ms",
      "999ms",
      "1.0s",
      "1.3s",
      "42s"
    ]);
    expect(formatMs(-1)).toBe("0ms");
  });
});

describe("formatStages", () => {
  it("prints each stage in the order the server sent them, then the total", () => {
    expect(
      formatStages([
        { stage: "admit", ms: 12 },
        { stage: "inflate", ms: 340 },
        { stage: "store", ms: 1248 }
      ])
    ).toBe("admit 12ms · inflate 340ms · store 1.2s (total 1.6s)");
  });

  it("prints nothing for a server that sent no timings", () => {
    expect(formatStages(undefined)).toBeUndefined();
    expect(formatStages([])).toBeUndefined();
  });
});

describe("refusalHeadline", () => {
  it("names the stage the server named, and only then", () => {
    expect(refusalHeadline(422, "inflate")).toBe("publish failed (422) at the inflate stage");
    expect(refusalHeadline(422, null)).toBe("publish failed (422)");
    expect(refusalHeadline(422, "  ")).toBe("publish failed (422)");
  });
});

describe("uploadBundle", () => {
  it("answers the stage timings a publish returns", async () => {
    const stages = [
      { stage: "admit", ms: 5 },
      { stage: "after", ms: 9 }
    ];
    stubFetch(TARGET, {
      "POST /api/customer-apps/publish": () => ({
        status: 200,
        body: { app_id: "a", build_id: "b", url: "/u", channel: "draft", stages }
      })
    });
    expect((await upload()).stages).toEqual(stages);
  });

  it("puts the stage a refused publish failed at on the error line", async () => {
    stubFetch(TARGET, {
      "POST /api/customer-apps/publish": () => ({
        status: 422,
        text: "invalid bundle: not gzip",
        headers: { [PUBLISH_STAGE_HEADER]: "inflate" }
      })
    });
    const cause = await upload().catch((thrown: unknown) => thrown);
    expect(cause).toBeInstanceOf(CliError);
    expect((cause as CliError).message).toBe("publish failed (422) at the inflate stage");
    expect((cause as CliError).detail).toBe("invalid bundle: not gzip");
  });
});
