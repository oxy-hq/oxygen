import { describe, expect, it } from "vitest";
import type { CompileStatus, RevisionSummary } from "@/services/api/compile";
import { deriveView } from "./deriveCompileView";

/**
 * Regression tests for the freshness verdict.
 *
 * The badge's resting label is the one thing a person reads to answer "is my
 * change live?". It previously answered by comparing the compiled revision
 * against the *working copy* HEAD — but compiles are taken from that same ref,
 * so the comparison was circular and reported "Up to date" for a workspace
 * sitting behind origin. See oxygen-workspace-sync-bugs.md bug 3.
 */

const READY: RevisionSummary = {
  revision_id: "rev-1",
  status: "ready",
  kind: "main",
  branch: "main",
  git_sha: "0c9ad8f0000000000000000000000000000000000",
  started_at: "2026-07-21T09:40:00Z",
  finished_at: "2026-07-21T09:40:44Z",
  duration_ms: 44000
};

function status(over: Partial<CompileStatus> = {}): CompileStatus {
  return {
    workspace_id: "ws-1",
    current_revision_id: "rev-1",
    latest: READY,
    can_compile: true,
    head_sha: "0c9ad8f0000000000000000000000000000000000",
    compiled_sha: "0c9ad8f0000000000000000000000000000000000",
    remote_sha: "0c9ad8f0000000000000000000000000000000000",
    remote_fetched_at: new Date().toISOString(),
    compiled_ahead: 0,
    compiled_behind: 0,
    default_branch: "main",
    boundary_active: true,
    ...over
  };
}

describe("deriveView", () => {
  it("reports fresh only when the serving revision matches the origin tip", () => {
    const view = deriveView(status());
    expect(view.kind).toBe("fresh");
    expect(view.verb).toBe("Up to date");
  });

  it("does NOT report fresh when origin has moved past the serving revision", () => {
    // The exact shape from the incident: local HEAD and the compiled revision
    // agree (0c9ad8f) because the compile was taken from that HEAD — while the
    // fix everyone was waiting for sits on origin as cbe3089.
    const view = deriveView(status({ remote_sha: "cbe3089000000000000000000000000000000000" }));
    expect(view.kind).toBe("stale");
    expect(view.verb).not.toBe("Up to date");
    // The chip must point at what's missing, not at the already-served SHA.
    expect(view.sha).toContain("cbe3089");
  });

  it("does not claim freshness when the remote tip is unknown", () => {
    const view = deriveView(status({ remote_sha: null }));
    expect(view.kind).toBe("unverified");
    expect(view.verb).not.toBe("Up to date");
  });

  it("does not claim freshness from a stale fetch", () => {
    // Matching SHAs, but the tracking ref is an hour old — origin could have
    // moved and this clone would not know. Silence beats a false green tick.
    const anHourAgo = new Date(Date.now() - 60 * 60 * 1000).toISOString();
    const view = deriveView(status({ remote_fetched_at: anHourAgo }));
    expect(view.kind).toBe("unverified");
  });

  it("treats a never-fetched clone as unverified, not up to date", () => {
    expect(deriveView(status({ remote_fetched_at: null })).kind).toBe("unverified");
  });

  it("reports never when nothing has been promoted yet", () => {
    expect(deriveView(status({ compiled_sha: null, current_revision_id: null })).kind).toBe(
      "never"
    );
  });

  it("ignores head_sha entirely when deciding freshness", () => {
    // Local commits that were never pushed move head_sha away from both the
    // compiled and remote SHAs. That is a push/pull concern, not a freshness
    // one: what is served still matches origin, so this stays fresh.
    const view = deriveView(status({ head_sha: "aaaaaaa0000000000000000000000000000000000" }));
    expect(view.kind).toBe("fresh");
  });

  it("surfaces in-flight and failed compiles ahead of any freshness verdict", () => {
    expect(deriveView(status({ latest: { ...READY, status: "compiling" } })).kind).toBe(
      "compiling"
    );
    expect(deriveView(status({ latest: { ...READY, status: "failed" } })).kind).toBe("failed");
  });

  it("does not call a revision ahead of origin 'stale'", () => {
    // Every restore mints a never-pushed "Restore to X" commit, and restore
    // now auto-compiles — so the serving revision routinely contains commits
    // origin lacks. Bare SHA inequality would flag that as behind and tell the
    // operator to compile toward an OLDER origin SHA.
    const view = deriveView(
      status({
        compiled_sha: "1111111000000000000000000000000000000000",
        remote_sha: "cbe3089000000000000000000000000000000000",
        compiled_ahead: 1,
        compiled_behind: 0
      })
    );
    expect(view.kind).toBe("ahead");
    expect(view.verb).toBe("Up to date");
  });

  it("treats a diverged revision as stale — origin still has unshipped commits", () => {
    const view = deriveView(
      status({
        compiled_sha: "1111111000000000000000000000000000000000",
        remote_sha: "cbe3089000000000000000000000000000000000",
        compiled_ahead: 1,
        compiled_behind: 2
      })
    );
    expect(view.kind).toBe("stale");
  });

  it("falls back to stale when ancestry is unknown", () => {
    // Erring toward "there may be something unshipped" prompts a compile;
    // erring the other way would assert everything is live without knowing.
    const view = deriveView(
      status({
        compiled_sha: "1111111000000000000000000000000000000000",
        remote_sha: "cbe3089000000000000000000000000000000000",
        compiled_ahead: null,
        compiled_behind: null
      })
    );
    expect(view.kind).toBe("stale");
  });

  describe("a serving revision that is not known to match the working copy", () => {
    // The workspace this was found on: it served a snapshot of its disk
    // compiled months earlier, the files had since been renamed, and the
    // server reported 0 ahead / 0 behind for a SHA git cannot compare.
    const snapshot = {
      compiled_sha: "local-ce83e24f-0000-0000-0000-000000000000",
      head_sha: "91d0c74f00000000000000000000000000000000",
      remote_sha: "2cb1956700000000000000000000000000000000",
      compiled_ahead: null,
      compiled_behind: null,
      compiled_matches_head: false
    };

    it("is out of date, pointing at the working copy's commit", () => {
      const view = deriveView(status(snapshot));
      expect(view.kind).toBe("stale");
      expect(view.verb).toBe("Compile");
      expect(view.sha).toBe("91d0c74 ↑");
    });

    it("is out of date even when origin cannot be checked", () => {
      // Without the flag this rests on a neutral "Compiled local-c…", which
      // is how the snapshot went unnoticed.
      expect(deriveView(status({ ...snapshot, remote_sha: null })).kind).toBe("stale");
      const anHourAgo = new Date(Date.now() - 60 * 60 * 1000).toISOString();
      expect(deriveView(status({ ...snapshot, remote_fetched_at: anHourAgo })).kind).toBe("stale");
    });

    it("is not mistaken for a revision ahead of origin", () => {
      const view = deriveView(status({ ...snapshot, compiled_ahead: 1, compiled_behind: 0 }));
      expect(view.kind).toBe("stale");
    });

    it("stays fresh when it is level with origin and only this clone is behind", () => {
      // Compiled from the commit origin is on; the working copy has not
      // pulled it. What was merged is live, so there is nothing to compile.
      const view = deriveView(
        status({
          head_sha: "0c9ad8f0000000000000000000000000000000000",
          compiled_sha: "cbe3089000000000000000000000000000000000",
          remote_sha: "cbe3089000000000000000000000000000000000",
          compiled_matches_head: false
        })
      );
      expect(view.kind).toBe("fresh");
    });

    it("does not prompt a compile when a newer commit is served than this clone has", () => {
      // Origin moved to H and H was compiled and promoted before this clone
      // fetched: head and the cached origin tip are both still the old commit,
      // and git here has never seen H. "Compile" would ship the old commit
      // over the new one.
      const view = deriveView(
        status({
          head_sha: "0c9ad8f0000000000000000000000000000000000",
          remote_sha: "0c9ad8f0000000000000000000000000000000000",
          compiled_sha: "cbe3089000000000000000000000000000000000",
          compiled_ahead: null,
          compiled_behind: null,
          compiled_matches_head: false
        })
      );
      expect(view.kind).toBe("unverified");
      expect(view.verb).toBe("Compiled");
      expect(view.sha).toBe("cbe3089");
    });

    it("still reports a placeable commit that origin has moved past as stale", () => {
      const view = deriveView(
        status({
          head_sha: "cbe3089000000000000000000000000000000000",
          remote_sha: "cbe3089000000000000000000000000000000000",
          compiled_sha: "0c9ad8f0000000000000000000000000000000000",
          compiled_ahead: 0,
          compiled_behind: 2,
          compiled_matches_head: false
        })
      );
      expect(view.kind).toBe("stale");
      expect(view.sha).toBe("cbe3089 ↑");
    });

    it("changes nothing for a server that does not send the flag", () => {
      const { compiled_matches_head: _unsent, ...older } = snapshot;
      expect(deriveView(status({ ...older, remote_sha: null })).kind).toBe("unverified");
    });
  });

  it("collapses to no-git for blank / no-remote workspaces", () => {
    expect(deriveView(status({ head_sha: null })).kind).toBe("no-git");
  });
});
