// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppLifecycle } from "@/hooks/api/customApps/deriveLifecycle";
import type { AppBuild } from "@/types/apps";
import { lifecycleVerdict } from "../../appStatus";
import type { LifecycleActions } from "./actions";
import { LifecycleCard } from "./LifecycleCard";
import { LIFECYCLE_COPY } from "./lifecycleCopy";

afterEach(cleanup);

const build = (over: Partial<AppBuild>): AppBuild => ({
  id: over.id ?? "b",
  build_id: over.build_id ?? "0123456789abcdef",
  created_at: "2026-09-19T17:00:00Z",
  is_draft: false,
  is_published: false,
  ...over
});

const LIVE = build({ id: "live", build_id: "aaaaaaaaaaaaaaaa", is_published: true });
const DRAFT = build({ id: "draft", build_id: "bbbbbbbbbbbbbbbb", is_draft: true });
const OLD = build({ id: "old", build_id: "cccccccccccccccc" });

const lifecycle = (over: Partial<AppLifecycle> = {}): AppLifecycle => ({
  liveBuild: LIVE,
  draftBuild: DRAFT,
  draftAhead: true,
  published: true,
  publishedAt: "2026-09-19T17:30:27Z",
  verdict: lifecycleVerdict({ published: true, health: "operational" }),
  heldCount: 2,
  heldTruncated: false,
  lastPromoted: { at: "2026-09-19T17:30:27Z", byEmail: "ops@oxy.tech" },
  builds: [DRAFT, LIVE, OLD],
  isLoading: false,
  error: null,
  ...over
});

const actions = (over: Partial<LifecycleActions> = {}): LifecycleActions => ({
  promoteDraft: vi.fn(async () => undefined),
  publish: vi.fn(async () => undefined),
  rollbackTo: vi.fn(async () => undefined),
  unpublish: vi.fn(async () => undefined),
  pending: false,
  ...over
});

const renderCard = (l = lifecycle(), a = actions()) =>
  render(<LifecycleCard lifecycle={l} actions={a} orgSlug='acme' />);

describe("LifecycleCard — what is live", () => {
  it("names the live build, who promoted it, and the draft ahead of it", () => {
    renderCard();
    expect(screen.getByTestId("app-lifecycle-live").textContent).toContain("aaaaaaa");
    expect(screen.getByTestId("app-lifecycle-live").textContent).toContain("ops@oxy.tech");
    expect(screen.getByTestId("app-lifecycle-draft").textContent).toContain("bbbbbbb");
    expect(screen.getByTestId("app-lifecycle-draft").textContent).toContain("ahead of production");
    expect(screen.getByTestId("app-lifecycle-held").textContent).toBe("2 writes held on staging");
  });

  it("says the draft is the live build when nothing is ahead", () => {
    renderCard(lifecycle({ draftAhead: false }));
    expect(screen.getByTestId("app-lifecycle-draft").textContent).toBe("Same as production");
    expect(screen.queryByTestId("app-lifecycle-promote")).toBeNull();
  });

  it("shows no held line when the count is unknown — never a zero", () => {
    renderCard(lifecycle({ heldCount: null }));
    expect(screen.queryByTestId("app-lifecycle-held")).toBeNull();
  });
});

describe("LifecycleCard — the verbs", () => {
  it("promotes the draft only after the shared confirm", async () => {
    const a = actions();
    renderCard(lifecycle(), a);
    await userEvent.click(screen.getByTestId("app-lifecycle-promote"));
    expect(a.promoteDraft).not.toHaveBeenCalled();

    const copy = LIFECYCLE_COPY.promote("bbbbbbbbbbbbbbbb", "aaaaaaaaaaaaaaaa");
    expect(screen.getByTestId("app-lifecycle-confirm").textContent).toContain(copy.title);
    expect(screen.getByTestId("app-lifecycle-confirm").textContent).toContain(copy.description);

    await userEvent.click(screen.getByTestId("app-lifecycle-confirm-ok"));
    expect(a.promoteDraft).toHaveBeenCalledTimes(1);
  });

  it("rolls back to a chosen retained build, never the live one", async () => {
    const a = actions();
    renderCard(lifecycle(), a);
    await userEvent.click(screen.getByTestId("app-lifecycle-rollback"));
    expect(screen.queryByTestId("app-lifecycle-rollback-aaaaaaa")).toBeNull();
    await userEvent.click(await screen.findByTestId("app-lifecycle-rollback-ccccccc"));
    await userEvent.click(screen.getByTestId("app-lifecycle-confirm-ok"));
    expect(a.rollbackTo).toHaveBeenCalledWith(OLD);
  });

  it("offers Publish, not Promote, to a draft — and Unpublish only to a published app", () => {
    renderCard(lifecycle({ published: false, liveBuild: null, builds: [DRAFT] }));
    expect(screen.getByTestId("app-lifecycle-publish")).toBeEnabled();
    expect(screen.queryByTestId("app-lifecycle-promote")).toBeNull();
    expect(screen.queryByTestId("app-lifecycle-unpublish")).toBeNull();
    cleanup();

    renderCard();
    expect(screen.getByTestId("app-lifecycle-unpublish")).toBeEnabled();
    expect(screen.queryByTestId("app-lifecycle-publish")).toBeNull();
  });

  it("renders no control for a verb the surface did not provide", () => {
    renderCard(lifecycle(), actions({ rollbackTo: undefined, promoteDraft: undefined }));
    expect(screen.queryByTestId("app-lifecycle-rollback")).toBeNull();
    expect(screen.queryByTestId("app-lifecycle-promote")).toBeNull();
    expect(screen.getByTestId("app-lifecycle-unpublish")).toBeEnabled();
  });

  it("keeps the confirm open when the verb fails, so the operator can retry", async () => {
    const a = actions({ unpublish: vi.fn(async () => Promise.reject(new Error("nope"))) });
    renderCard(lifecycle(), a);
    await userEvent.click(screen.getByTestId("app-lifecycle-unpublish"));
    await userEvent.click(screen.getByTestId("app-lifecycle-confirm-ok"));
    expect(a.unpublish).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("app-lifecycle-confirm")).toBeVisible();
  });
});
