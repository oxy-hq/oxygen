// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RevisionFilesService } from "@/services/api/revisionFiles";
import { githubCommitUrl } from "../githubRepoUrl";
import RevisionInfo, { compiledAgo } from "./RevisionInfo";

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "ws" }, branchName: "" })
}));
vi.mock("@/services/api/axios", () => ({ apiClient: {} }));

const renderInfo = (remoteUrl?: string) =>
  render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
    >
      <RevisionInfo remoteUrl={remoteUrl} />
    </QueryClientProvider>
  );

const served = {
  revision_id: "rev-1",
  git_sha: "0123456789abcdef",
  branch: "main",
  kind: "main",
  compiled_at: new Date(Date.now() - 3 * 3600 * 1000).toISOString(),
  file_count: 78
};

beforeEach(() => {
  vi.spyOn(RevisionFilesService, "served").mockResolvedValue(served);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("RevisionInfo", () => {
  it("shows the branch, the short commit linked to GitHub, and when it compiled", async () => {
    renderInfo("git@github.com:acme/analytics.git");
    const info = await screen.findByTestId("ide-revision-info");
    expect(info.textContent).toContain("main");
    expect(info.textContent).toContain("compiled 3 hours ago");
    const link = screen.getByRole("link", { name: "0123456" });
    expect(link.getAttribute("href")).toBe(
      "https://github.com/acme/analytics/commit/0123456789abcdef"
    );
  });

  it("marks a preview revision and shows the commit unlinked off GitHub", async () => {
    vi.spyOn(RevisionFilesService, "served").mockResolvedValue({ ...served, kind: "preview" });
    renderInfo("https://gitlab.com/acme/analytics.git");
    const info = await screen.findByTestId("ide-revision-info");
    expect(info.textContent).toContain("preview");
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("says a revision compiled from local files has no commit, rather than a fake hash", async () => {
    vi.spyOn(RevisionFilesService, "served").mockResolvedValue({
      ...served,
      git_sha: "local-e61706c8-3e1d-468b-aeb5-92b3152bc741",
      branch: null
    });
    renderInfo("git@github.com:acme/analytics.git");
    const info = await screen.findByTestId("ide-revision-info");
    expect(info.textContent).toContain("local files, no commit");
    expect(info.textContent).not.toContain("local-e");
    expect(screen.queryByRole("link")).toBeNull();
  });

  const httpError = (status: number) =>
    Object.assign(new Error(`status ${status}`), { isAxiosError: true, response: { status } });

  it("says nothing is compiled when the workspace serves no revision (503)", async () => {
    vi.spyOn(RevisionFilesService, "served").mockRejectedValue(httpError(503));
    renderInfo();
    expect(await screen.findByTestId("ide-revision-none")).toBeTruthy();
  });

  it("does not call a server failure 'nothing compiled'", async () => {
    vi.spyOn(RevisionFilesService, "served").mockRejectedValue(httpError(500));
    renderInfo();
    expect(await screen.findByTestId("ide-revision-unavailable")).toBeTruthy();
    expect(screen.queryByTestId("ide-revision-none")).toBeNull();
  });
});

describe("compiledAgo", () => {
  it("reads a missing or bad time as nothing", () => {
    expect(compiledAgo(null)).toBe("");
    expect(compiledAgo("not a date")).toBe("");
  });

  it("does not claim a future time", () => {
    const now = new Date("2026-11-13T07:00:00Z");
    expect(compiledAgo("2026-11-13T07:00:05Z", now)).toBe("just now");
  });
});

describe("githubCommitUrl", () => {
  it("links a GitHub remote's commit and nothing else", () => {
    expect(githubCommitUrl("https://github.com/acme/a.git", "abc")).toBe(
      "https://github.com/acme/a/commit/abc"
    );
    expect(githubCommitUrl("https://gitlab.com/acme/a.git", "abc")).toBeNull();
    expect(githubCommitUrl(null, "abc")).toBeNull();
  });
});
