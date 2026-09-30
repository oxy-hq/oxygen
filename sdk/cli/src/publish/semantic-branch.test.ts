import { afterEach, describe, expect, it, vi } from "vitest";
import { compileSemanticBranch } from "./semantic-branch.js";

type Answer = { git_sha: string; status: string; revision_id?: string; task_id?: string };

function stubServer(answers: Answer[]) {
  const calls: { method: string; url: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: { method: string }) => {
      calls.push({ method: init.method, url });
      const next = answers.shift();
      if (!next) throw new Error(`unexpected ${init.method} ${url}`);
      return new Response(JSON.stringify(next), { status: 200 });
    })
  );
  return calls;
}

afterEach(() => vi.unstubAllGlobals());

describe("compileSemanticBranch", () => {
  // Fixtures use only shapes the server produces: an enqueuing POST carries a
  // `task_id`, a POST never answers `stale`, and only a POST that JOINED a
  // compile (`compiling`, no `task_id`) leaves a re-POST to make.
  it("re-POSTs once when it joined a compile whose result it can't reuse", async () => {
    const calls = stubServer([
      { git_sha: "abc", status: "compiling" },
      { git_sha: "abc", status: "stale" },
      { git_sha: "abc", status: "pending", task_id: "t1" },
      { git_sha: "abc", status: "ready", revision_id: "rev-1" }
    ]);
    const rev = await compileSemanticBranch("https://oxy.test", "t", "proj", "feat/x", {
      pollMs: 0
    });
    expect(rev).toBe("rev-1");
    expect(calls.map((c) => c.method)).toEqual(["POST", "GET", "POST", "GET"]);
  });

  it("re-POSTs at most once, then polls while the new compile is queued", async () => {
    const calls = stubServer([
      { git_sha: "abc", status: "compiling" },
      { git_sha: "abc", status: "stale" },
      { git_sha: "abc", status: "pending", task_id: "t2" },
      { git_sha: "abc", status: "stale" },
      { git_sha: "abc", status: "stale" },
      { git_sha: "abc", status: "compiling" },
      { git_sha: "abc", status: "ready", revision_id: "rev-3" }
    ]);
    const rev = await compileSemanticBranch("https://oxy.test", "t", "proj", "feat/x", {
      pollMs: 0
    });
    expect(rev).toBe("rev-3");
    expect(calls.map((c) => c.method)).toEqual(["POST", "GET", "POST", "GET", "GET", "GET", "GET"]);
  });

  it("never re-POSTs when its own POST enqueued the compile", async () => {
    const calls = stubServer([
      { git_sha: "abc", status: "pending", task_id: "t1" },
      { git_sha: "abc", status: "stale" },
      { git_sha: "abc", status: "stale" },
      { git_sha: "abc", status: "ready", revision_id: "rev-4" }
    ]);
    await compileSemanticBranch("https://oxy.test", "t", "proj", "feat/x", { pollMs: 0 });
    expect(calls.map((c) => c.method)).toEqual(["POST", "GET", "GET", "GET"]);
  });

  it("never re-POSTs a merely queued compile", async () => {
    const calls = stubServer([
      { git_sha: "abc", status: "pending", task_id: "t3" },
      { git_sha: "abc", status: "pending" },
      { git_sha: "abc", status: "ready", revision_id: "rev-2" }
    ]);
    await compileSemanticBranch("https://oxy.test", "t", "proj", "feat/x", { pollMs: 0 });
    expect(calls.map((c) => c.method)).toEqual(["POST", "GET", "GET"]);
  });
});
