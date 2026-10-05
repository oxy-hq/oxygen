// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CustomApp } from "@/types/apps";
import { PREVIEW_NONCE_PARAM } from "./appViewState";
import { DetailToolbar } from "./components/DetailToolbar";
import { LivePreview } from "./components/LivePreview";
import {
  DETAIL_LOAD_FAILED,
  type DraftTarget,
  defaultChannel,
  draftTarget,
  liveView,
  NO_STAGING_HOST,
  NOT_PROMOTED_YET,
  STAGING_FRAME_NOTE,
  STAGING_NOT_CONFIGURED
} from "./draftTarget";

/**
 * The console's Draft view is the app's staging host, and nothing else.
 *
 * The staff `oxy_preview_draft` cookie that flipped the production URL to the draft
 * is retired, so Draft can no longer mean "the same URL, a different build": it frames
 * `staging_url`, and when there is none it says why and frames nothing. Above all it
 * never loads the production URL under a "draft" label.
 */

// The toolbar's assume-role and org-home buttons fetch on mount; neither is what
// these cases are about.
vi.mock("./components/DetailToolbar/ActAsOrgButton", () => ({ ActAsOrgButton: () => null }));
vi.mock("./components/DetailToolbar/OrgHomeButton", () => ({ OrgHomeButton: () => null }));

// The staging banner (mounted by LivePreview whenever the frame is staging)
// fetches via react-query, which needs a QueryClientProvider this file's bare
// `render()` doesn't set up. Its own behavior is covered by
// `StagingBanner.test.tsx`; here it is a held-count stub so these cases stay
// about Draft target resolution, not the banner's fetch state.
vi.mock("@/hooks/api/customApps/useCustomApps", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/hooks/api/customApps/useCustomApps")>();
  return { ...actual, useStagingHeld: () => ({ data: [], error: null, isLoading: false }) };
});

afterEach(cleanup);

const PRODUCTION = "http://localhost:3000/customer-apps/acme/store/";
const STAGING = "https://staging--acme--store.customer-apps.oxygen-hq.com/";

const app = (over: Partial<CustomApp> = {}): CustomApp =>
  ({
    id: "app-1",
    slug: "store",
    name: "Store",
    org_slug: "acme",
    url: PRODUCTION,
    url_subdomain: null,
    staging_url: null,
    published_at: "2026-09-01T00:00:00Z",
    ...over
  }) as unknown as CustomApp;

const renderPreview = (draft: DraftTarget, path: string | null = null) =>
  render(
    <LivePreview
      app={app({ staging_url: draft.kind === "staging" ? draft.url : null })}
      device='desktop'
      channel='draft'
      draft={draft}
      nonce={3}
      path={path}
      onPathChange={() => undefined}
    />
  );

const NEVER_PUBLISHED = { published_at: null } as unknown as Partial<CustomApp>;

const renderToolbar = (draft: DraftTarget, over: Partial<CustomApp> = {}) =>
  render(
    <DetailToolbar
      app={app(over)}
      tab='preview'
      device='desktop'
      channel='published'
      draft={draft}
      showTabs={false}
      onTabChange={() => undefined}
      onDeviceChange={() => undefined}
      onChannelChange={() => undefined}
      onReload={() => undefined}
    />
  );

describe("draftTarget", () => {
  it("points at the staging host when the detail names one", () => {
    expect(draftTarget({ staging_url: STAGING, url_subdomain: "https://acme--store.x/" })).toEqual({
      kind: "staging",
      url: STAGING
    });
  });

  it("says staging isn't configured when the deployment has no customer-apps zone", () => {
    expect(draftTarget({ staging_url: null, url_subdomain: null })).toEqual({
      kind: "unavailable",
      reason: STAGING_NOT_CONFIGURED
    });
  });

  it("says this app has no staging host when the zone exists but names none", () => {
    expect(
      draftTarget({ staging_url: null, url_subdomain: "https://acme--store.customer-apps.x/" })
    ).toEqual({ kind: "unavailable", reason: NO_STAGING_HOST });
  });

  it("is pending until the detail response lands", () => {
    expect(draftTarget(null)).toEqual({ kind: "pending" });
  });

  it("is unavailable with a reason, not pending for ever, when the detail fetch fails", () => {
    expect(draftTarget(null, true)).toEqual({ kind: "unavailable", reason: DETAIL_LOAD_FAILED });
  });
});

describe("the live view of an app never promoted", () => {
  it("is Live with the real-writes copy, and promoted apps stay Published", () => {
    expect(liveView({ published_at: null })).toEqual({ label: "Live", note: NOT_PROMOTED_YET });
    expect(liveView({ published_at: "2026-09-01T00:00:00Z" })).toEqual({
      label: "Published",
      note: null
    });
  });

  it("never calls that build draft, staging or held", () => {
    expect(NOT_PROMOTED_YET).not.toMatch(/draft|staging|held/i);
  });

  it("opens on staging when there is a host, waits while it loads, else opens Live", () => {
    const never = { published_at: null };
    expect(defaultChannel(never, { kind: "staging", url: STAGING })).toBe("draft");
    expect(defaultChannel(never, { kind: "pending" })).toBe("draft");
    expect(defaultChannel(never, { kind: "unavailable", reason: NO_STAGING_HOST })).toBe(
      "published"
    );
    expect(defaultChannel({ published_at: "2026-09-01T00:00:00Z" }, { kind: "pending" })).toBe(
      "published"
    );
  });

  it("frames the production URL and says its writes are real", () => {
    const { container } = render(
      <LivePreview
        app={app(NEVER_PUBLISHED)}
        device='desktop'
        channel='published'
        draft={{ kind: "unavailable", reason: STAGING_NOT_CONFIGURED }}
        nonce={1}
        path={null}
        onPathChange={() => undefined}
      />
    );
    const src = container.querySelector("iframe")?.getAttribute("src") ?? "";
    expect(src.startsWith(PRODUCTION)).toBe(true);
    expect(screen.getByTestId("admin-app-frame-note").textContent).toBe(NOT_PROMOTED_YET);
  });
});

describe("Draft in the preview stage", () => {
  it("frames the staging URL, not the production one", () => {
    const { container } = renderPreview({ kind: "staging", url: STAGING });
    const src = container.querySelector("iframe")?.getAttribute("src") ?? "";
    expect(new URL(src).origin).toBe(new URL(STAGING).origin);
    expect(src.startsWith(STAGING)).toBe(true);
    expect(src).not.toContain("/customer-apps/acme/store/");
  });

  it("frames nothing and says why when there is no staging host", () => {
    const { container } = renderPreview({ kind: "unavailable", reason: STAGING_NOT_CONFIGURED });
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByTestId("admin-app-draft-notice").textContent).toContain(
      STAGING_NOT_CONFIGURED
    );
  });

  it("starts the staging frame at the ?preview= deep link, once per document", () => {
    const { container, rerender } = renderPreview(
      { kind: "staging", url: STAGING },
      "/vendors?id=7"
    );
    const first = new URL(container.querySelector("iframe")?.getAttribute("src") ?? "");
    expect(first.origin).toBe(new URL(STAGING).origin);
    expect(first.pathname).toBe("/vendors");
    expect(first.searchParams.get("id")).toBe("7");
    expect(first.searchParams.get(PREVIEW_NONCE_PARAM)).not.toBeNull();

    // A later `path` must not swap `src` and reload the frame under the operator.
    rerender(
      <LivePreview
        app={app({ staging_url: STAGING })}
        device='desktop'
        channel='draft'
        draft={{ kind: "staging", url: STAGING }}
        nonce={3}
        path='/elsewhere'
        onPathChange={() => undefined}
      />
    );
    expect(container.querySelector("iframe")?.getAttribute("src")).toBe(first.toString());
  });

  it("says under a staging frame that the request log and navigation can't follow it", () => {
    renderPreview({ kind: "staging", url: STAGING });
    expect(screen.getByTestId("admin-app-frame-note").textContent).toBe(STAGING_FRAME_NOTE);
  });

  it("says why when the detail fetch failed, instead of looking up for ever", () => {
    const { container } = renderPreview(draftTarget(null, true));
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByTestId("admin-app-draft-notice").textContent).toContain(DETAIL_LOAD_FAILED);
  });

  it("frames nothing while the staging host is still being looked up", () => {
    const { container } = renderPreview({ kind: "pending" });
    expect(container.querySelector("iframe")).toBeNull();
  });
});

describe("the Draft control", () => {
  it("is disabled with no staging_url, and reads why", async () => {
    renderToolbar(draftTarget({ staging_url: null, url_subdomain: null }));
    const control = screen.getByTestId("admin-app-channel-draft");
    expect(control).toBeDisabled();

    await userEvent.hover(control.parentElement as HTMLElement);
    expect((await screen.findByRole("tooltip")).textContent).toContain(STAGING_NOT_CONFIGURED);
  });

  it("is enabled when the app has a staging host", () => {
    renderToolbar({ kind: "staging", url: STAGING });
    expect(screen.getByTestId("admin-app-channel-draft")).toBeEnabled();
  });

  it("still opens staging for an app never promoted", () => {
    renderToolbar({ kind: "staging", url: STAGING }, NEVER_PUBLISHED);
    expect(screen.getByTestId("admin-app-channel-draft")).toBeEnabled();
  });
});

describe("the Published control", () => {
  it("is enabled as Live for an app never promoted with no staging host, and says why", async () => {
    renderToolbar(draftTarget({ staging_url: null, url_subdomain: null }), NEVER_PUBLISHED);
    const control = screen.getByTestId("admin-app-channel-published");
    expect(control).toBeEnabled();
    expect(control.textContent).toBe("Live");
    expect(screen.getByTestId("admin-app-channel-draft")).toBeDisabled();

    await userEvent.hover(control);
    expect((await screen.findByRole("tooltip")).textContent).toContain(NOT_PROMOTED_YET);
  });

  it("reads Published for a promoted app", () => {
    renderToolbar({ kind: "staging", url: STAGING });
    expect(screen.getByTestId("admin-app-channel-published").textContent).toBe("Published");
  });
});
