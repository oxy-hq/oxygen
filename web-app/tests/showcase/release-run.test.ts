// What a release does with one shipped PR. GitHub and the capture pipeline are
// the boundaries mocked; the decision between them is what is under test.

import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMeter } from "../agentic/runner/budget";
import { findShowcaseComment, prFacts, upsertShowcaseComment } from "./github";
import { showcasePr } from "./pipeline";
import { renderComment } from "./record";
import { type ReleaseTarget, releaseOne } from "./release-run";
import { uploadToThread } from "./slack";
import type { Outcome, RecordPointer, ShowcaseRecord } from "./types";

vi.mock("./github", async (original) => ({
  ...(await original<typeof import("./github")>()),
  prFacts: vi.fn(),
  findShowcaseComment: vi.fn(),
  upsertShowcaseComment: vi.fn()
}));
vi.mock("./pipeline", () => ({ showcasePr: vi.fn() }));
vi.mock("./slack", () => ({ uploadToThread: vi.fn() }));

const env = {
  repo: "o/r",
  apiKey: "k",
  baseUrl: "http://x",
  backendUrl: "http://x",
  databaseUrl: ""
};
const item = { pr: 7, type: "feat" as const, text: "Pause automations" };
const THREAD = "1790000000.000001";

function record(outcome: Outcome, cost_usd = 0.2): ShowcaseRecord {
  return {
    version: 1,
    pr: 7,
    title: "feat: pause automations",
    head_sha: "abc",
    outcome,
    reason: `it ended ${outcome}`,
    plan: {
      verdict: "show",
      reason: "r",
      headline: "Automations can be paused",
      start_path: "/local",
      steps: [],
      expect: "A Paused badge",
      media: "screenshot"
    },
    screenshot: outcome === "captured" ? "media/screenshot.png" : undefined,
    cost_usd,
    captured_at: "2026-10-05T00:00:00Z"
  };
}

/** No Slack token: a dry run, so nothing here can post or write a PR comment. */
function target(totalUsd = 2): ReleaseTarget {
  return {
    channel: "C0000000",
    threadTs: THREAD,
    runId: "1",
    runUrl: "https://gh/run/1",
    featureBudgetUsd: 0.5,
    total: createMeter(totalUsd)
  };
}

function commentWith(pointer: Partial<RecordPointer>) {
  const full = {
    run_id: "9",
    artifact: "",
    head_sha: "abc",
    outcome: "rejected" as Outcome,
    ...pointer
  };
  return { id: 1, body: renderComment(record(full.outcome), full, "https://gh/run/9") };
}

/** A capture that leaves its screenshot where the release looks for it. */
function capturing(_env: unknown, _pr: unknown, dir: string) {
  mkdirSync(join(dir, "media"), { recursive: true });
  writeFileSync(join(dir, "media", "screenshot.png"), "png");
  return Promise.resolve(record("captured"));
}

let out: string;
beforeEach(() => {
  vi.resetAllMocks();
  out = mkdtempSync(join(tmpdir(), "showcase-release-"));
  vi.mocked(prFacts).mockReturnValue({
    title: "feat: pause automations",
    body: "",
    labels: [],
    files: ["web-app/src/pages/automations/index.tsx"],
    headSha: "abc"
  });
  vi.mocked(findShowcaseComment).mockReturnValue(undefined);
});

describe("releaseOne", () => {
  it("plans and captures a PR nothing ever looked at", async () => {
    vi.mocked(showcasePr).mockImplementation(capturing);
    const row = await releaseOne(env, target(), item, out);
    expect(showcasePr).toHaveBeenCalledOnce();
    expect(row).toMatchObject({ outcome: "captured", cost_usd: 0.2 });
  });

  // The fault this replaced: a PR that failed its one try at review time was
  // "decided at review time" and skipped, so three releases posted nothing.
  it.each<Outcome>(["rejected", "needs_seed", "over_budget", "not_visual", "failed"])(
    "plans again a PR an earlier run left %s",
    async (earlier) => {
      vi.mocked(findShowcaseComment).mockReturnValue(commentWith({ outcome: earlier }));
      vi.mocked(showcasePr).mockImplementation(capturing);
      const row = await releaseOne(env, target(), item, out);
      expect(showcasePr).toHaveBeenCalledOnce();
      expect(row.outcome).toBe("captured");
    }
  );

  // A release used to replay the recording a captured preview's artifact
  // carried, and post that artifact's media when the replay failed. Anyone who
  // can start a workflow can make such an artifact.
  it("plans a PR whose preview captured, too: nothing is read from a preview's artifact", async () => {
    vi.mocked(findShowcaseComment).mockReturnValue(
      commentWith({ outcome: "captured", artifact: "showcase-pr-7", run_id: "999" })
    );
    vi.mocked(showcasePr).mockImplementation(capturing);
    const row = await releaseOne(env, target(), item, out);
    expect(showcasePr).toHaveBeenCalledOnce();
    expect(row.outcome).toBe("captured");
  });

  it("posts nothing a capture did not end as captured, even with a picture lying there", async () => {
    vi.mocked(showcasePr).mockImplementation(async (_e, _pr, dir) => {
      mkdirSync(join(dir, "media"), { recursive: true });
      writeFileSync(join(dir, "media", "screenshot.png"), "a frame the judge rejected");
      return record("rejected");
    });
    const row = await releaseOne(env, target(), item, out);
    expect(row).toMatchObject({ outcome: "rejected" });
  });

  it("leaves the PR's record beside its media, for the run's artifact", async () => {
    vi.mocked(showcasePr).mockImplementation(capturing);
    await releaseOne(env, target(), item, out);
    const written = JSON.parse(readFileSync(join(out, "pr-7", "record.json"), "utf-8"));
    expect(written).toMatchObject({ pr: 7, outcome: "captured" });
  });

  // The first two prod releases on this code: four pictures captured, every
  // upload refused with not_in_channel, and each row read `failed … $0.000`
  // as if nothing had been tried or paid for.
  it("says when Slack refused a picture it captured, with what the capture cost", async () => {
    vi.mocked(showcasePr).mockImplementation(capturing);
    vi.mocked(uploadToThread).mockRejectedValue(
      new Error("Slack files.completeUploadExternal: not_in_channel")
    );
    const row = await releaseOne(env, { ...target(), slackToken: "xoxb-test" }, item, out);
    expect(row).toMatchObject({ outcome: "failed", cost_usd: 0.2 });
    expect(row.reason).toBe(
      "captured, not posted — Slack files.completeUploadExternal: not_in_channel"
    );
    expect(upsertShowcaseComment).not.toHaveBeenCalled();
  });

  it("marks the PR posted only once Slack has the picture", async () => {
    vi.mocked(showcasePr).mockImplementation(capturing);
    vi.mocked(uploadToThread).mockResolvedValue(undefined);
    const row = await releaseOne(env, { ...target(), slackToken: "xoxb-test" }, item, out);
    expect(row.outcome).toBe("captured");
    expect(upsertShowcaseComment).toHaveBeenCalledOnce();
    expect(vi.mocked(upsertShowcaseComment).mock.calls[0][2]).toContain(
      "Pictured under the release announcement"
    );
  });

  it("spends nothing on a PR with no screen, and says why", async () => {
    vi.mocked(prFacts).mockReturnValue({
      title: "feat: a route any replica serves",
      body: "",
      labels: [],
      files: ["crates/app/src/lib.rs"],
      headSha: "abc"
    });
    const row = await releaseOne(env, target(), item, out);
    expect(showcasePr).not.toHaveBeenCalled();
    expect(row).toMatchObject({ outcome: "not_candidate", reason: "no web-app screen changed" });
  });

  it("leaves out a PR labelled no-showcase", async () => {
    vi.mocked(prFacts).mockReturnValue({
      title: "feat: pause automations",
      body: "",
      labels: ["no-showcase"],
      files: ["web-app/src/pages/automations/index.tsx"],
      headSha: "abc"
    });
    vi.mocked(findShowcaseComment).mockReturnValue(
      commentWith({ outcome: "captured", artifact: "showcase-pr-7" })
    );
    const row = await releaseOne(env, target(), item, out);
    expect(showcasePr).not.toHaveBeenCalled();
    expect(row.outcome).toBe("not_candidate");
  });

  it("does not post twice in one thread", async () => {
    vi.mocked(findShowcaseComment).mockReturnValue(
      commentWith({ outcome: "captured", posted_in: [THREAD] })
    );
    const row = await releaseOne(env, target(), item, out);
    expect(showcasePr).not.toHaveBeenCalled();
    expect(row).toMatchObject({ outcome: "skipped", reason: "already posted in this thread" });
  });

  // Twenty rows of "over_budget: the next call could cost $0.231 and $0.156 is
  // left" read as twenty failures. They were one fact: the release's cap was spent.
  it("calls a PR the release's cap left no room to plan skipped, not over budget", async () => {
    vi.mocked(showcasePr).mockResolvedValue(record("over_budget", 0));
    const row = await releaseOne(env, target(0.15), item, out);
    expect(row).toMatchObject({
      outcome: "skipped",
      reason: "the release's spend limit is used up"
    });
  });

  it("still calls a PR that spent its own whole cap over budget", async () => {
    vi.mocked(showcasePr).mockImplementation((_e, _pr, _dir, meter) => {
      meter.spentUsd += 0.45;
      return Promise.resolve(record("over_budget", 0.45));
    });
    const row = await releaseOne(env, target(), item, out);
    expect(row).toMatchObject({ outcome: "over_budget", cost_usd: 0.45 });
  });

  it("stops at the release's cap, and charges each PR's spend to it", async () => {
    const t = target(0.3);
    vi.mocked(showcasePr).mockImplementation((_e, _pr, _dir, meter) => {
      meter.spentUsd += 0.3;
      return Promise.resolve(record("rejected", 0.3));
    });
    expect((await releaseOne(env, t, item, out)).outcome).toBe("rejected");
    expect(t.total.spentUsd).toBeCloseTo(0.3);
    const next = await releaseOne(env, t, { ...item, pr: 8 }, out);
    expect(showcasePr).toHaveBeenCalledOnce();
    expect(next).toMatchObject({
      outcome: "skipped",
      reason: "the release's spend limit is used up"
    });
  });
});
