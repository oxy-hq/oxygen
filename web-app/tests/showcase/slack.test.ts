import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { uploadToThread } from "./slack";

function fakeSlack(overrides: Record<string, unknown> = {}) {
  const calls: { url: string; body: unknown }[] = [];
  const f = (async (url: string, init?: RequestInit) => {
    calls.push({ url, body: init?.body });
    if (url.endsWith("files.getUploadURLExternal")) {
      const n = calls.filter((c) => c.url.endsWith("getUploadURLExternal")).length;
      return Response.json({ ok: true, upload_url: `https://upload.test/${n}`, file_id: `F${n}` });
    }
    if (url.startsWith("https://upload.test/")) return new Response("OK");
    return Response.json({ ok: true, ...overrides });
  }) as typeof fetch;
  return { f, calls };
}

const dir = mkdtempSync(join(tmpdir(), "slack-test-"));
const png = join(dir, "screenshot.png");
const mp4 = join(dir, "video.mp4");
writeFileSync(png, "png-bytes");
writeFileSync(mp4, "mp4");

const req = {
  token: "xoxb-test",
  channel: "C123",
  threadTs: "1700.1",
  comment: "*Headline*",
  files: [
    { path: png, title: "Headline" },
    { path: mp4, title: "Headline (video)" }
  ]
};

describe("uploadToThread", () => {
  it("uploads each file, then shares them all in the thread once", async () => {
    const { f, calls } = fakeSlack();
    await uploadToThread(req, f);
    expect(calls.map((c) => c.url)).toEqual([
      "https://slack.com/api/files.getUploadURLExternal",
      "https://upload.test/1",
      "https://slack.com/api/files.getUploadURLExternal",
      "https://upload.test/2",
      "https://slack.com/api/files.completeUploadExternal"
    ]);
    const first = calls[0].body as URLSearchParams;
    expect(first.get("filename")).toBe("screenshot.png");
    expect(first.get("length")).toBe("9");
    const done = calls[4].body as URLSearchParams;
    expect(done.get("channel_id")).toBe("C123");
    expect(done.get("thread_ts")).toBe("1700.1");
    expect(JSON.parse(done.get("files") ?? "[]")).toEqual([
      { id: "F1", title: "Headline" },
      { id: "F2", title: "Headline (video)" }
    ]);
  });

  it("fails on Slack's 200-with-ok:false, naming the error", async () => {
    const { f } = fakeSlack({ ok: false, error: "missing_scope" });
    await expect(uploadToThread({ ...req, files: [] }, f)).rejects.toThrow(
      "Slack files.completeUploadExternal: missing_scope"
    );
  });
});
