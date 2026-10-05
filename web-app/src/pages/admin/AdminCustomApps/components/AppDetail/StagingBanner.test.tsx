// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { STAGING_HELD_LIMIT } from "@/hooks/api/customApps/useCustomApps";
import type { StagingHeldEntry } from "@/services/api/customApps";
import type { CustomApp } from "@/types/apps";
import { LivePreview } from "./components/LivePreview";
import type { DraftTarget } from "./draftTarget";
import { qualifiedName, StagingBanner } from "./StagingBanner";

/**
 * What staging held, read back to the developer who caused it.
 *
 * Two things this file has to prove that aren't obvious from the component
 * alone: the exact copy (banner line, plural rule, empty state, the 404
 * state), and the mounting ruling from the staging-console plan — the banner
 * renders ONLY while the console is actually framing staging (channel
 * "draft" AND the target resolved to the staging host), never over the
 * production/Live frame and never with no staging host at all.
 */

type HeldResult = {
  data: StagingHeldEntry[] | undefined;
  error: unknown;
  isLoading: boolean;
};

let heldResult: HeldResult = { data: [], error: null, isLoading: false };

vi.mock("@/hooks/api/customApps/useCustomApps", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/hooks/api/customApps/useCustomApps")>();
  return { ...actual, useStagingHeld: () => heldResult };
});

afterEach(() => {
  cleanup();
  heldResult = { data: [], error: null, isLoading: false };
});

const write = (over: Partial<StagingHeldEntry["writes"][number]> = {}) => ({
  plane: "oltp",
  namespace: "app_orders",
  verb: "INSERT",
  table: "line_items",
  op: null,
  note: null,
  ...over
});

const entry = (over: Partial<StagingHeldEntry> = {}): StagingHeldEntry => ({
  at: new Date().toISOString(),
  function: "save",
  writes: [write()],
  ...over
});

describe("the held count and its plural", () => {
  it("reads 1 write (singular) for a single held row", () => {
    heldResult = { data: [entry()], error: null, isLoading: false };
    render(<StagingBanner appId='app-1' />);
    expect(screen.getByTestId("admin-app-staging-banner").textContent).toBe(
      "Staging · live reads · writes held or isolated · 1 write held"
    );
  });

  it("reads N writes (plural) for more than one", () => {
    heldResult = { data: [entry(), entry(), entry()], error: null, isLoading: false };
    render(<StagingBanner appId='app-1' />);
    expect(screen.getByTestId("admin-app-staging-banner").textContent).toBe(
      "Staging · live reads · writes held or isolated · 3 writes held"
    );
  });

  it("counts held writes, not held calls", () => {
    heldResult = {
      data: [entry({ writes: [write(), write({ table: "orders" })] }), entry()],
      error: null,
      isLoading: false
    };
    render(<StagingBanner appId='app-1' />);
    expect(screen.getByTestId("admin-app-staging-banner").textContent).toBe(
      "Staging · live reads · writes held or isolated · 3 writes held"
    );
  });

  it("reads N+ when the list came back as long as the limit asked for", () => {
    heldResult = {
      data: Array.from({ length: STAGING_HELD_LIMIT }, () => entry()),
      error: null,
      isLoading: false
    };
    render(<StagingBanner appId='app-1' />);
    expect(screen.getByTestId("admin-app-staging-banner-count").textContent).toBe(
      `${STAGING_HELD_LIMIT}+ writes held`
    );
  });

  it("reads 0 writes (plural) when nothing is held yet", () => {
    heldResult = { data: [], error: null, isLoading: false };
    render(<StagingBanner appId='app-1' />);
    expect(screen.getByTestId("admin-app-staging-banner").textContent).toBe(
      "Staging · live reads · writes held or isolated · 0 writes held"
    );
  });
});

describe("the popover rows", () => {
  it("lists time · function · plane verb namespace.table, one row per write", async () => {
    heldResult = {
      data: [
        entry({
          function: "sync-orders",
          writes: [
            write({ plane: "oltp", verb: "INSERT", namespace: "app_orders", table: "line_items" })
          ]
        })
      ],
      error: null,
      isLoading: false
    };
    render(<StagingBanner appId='app-1' />);

    await userEvent.click(screen.getByTestId("admin-app-staging-banner-count"));

    const rows = await screen.findAllByTestId("admin-app-staging-held-row");
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("sync-orders");
    expect(rows[0].textContent).toContain("oltp INSERT app_orders.line_items");
    expect(rows[0].textContent).toContain("just now");
  });

  it("gives a held call with several writes one row per write, sharing its time and function", async () => {
    heldResult = {
      data: [
        entry({
          function: "checkout",
          writes: [
            write({ plane: "oltp", verb: "INSERT", namespace: "app_orders", table: "orders" }),
            write({ plane: "warehouse", verb: "INSERT", namespace: "analytics", table: "events" })
          ]
        })
      ],
      error: null,
      isLoading: false
    };
    render(<StagingBanner appId='app-1' />);

    await userEvent.click(screen.getByTestId("admin-app-staging-banner-count"));

    const rows = await screen.findAllByTestId("admin-app-staging-held-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("checkout");
    expect(rows[0].textContent).toContain("oltp INSERT app_orders.orders");
    expect(rows[1].textContent).toContain("checkout");
    expect(rows[1].textContent).toContain("warehouse INSERT analytics.events");
  });

  it("keeps the server's newest-first order rather than re-sorting", async () => {
    heldResult = {
      data: [entry({ function: "newer-call" }), entry({ function: "older-call" })],
      error: null,
      isLoading: false
    };
    render(<StagingBanner appId='app-1' />);

    await userEvent.click(screen.getByTestId("admin-app-staging-banner-count"));

    const rows = await screen.findAllByTestId("admin-app-staging-held-row");
    expect(rows.map((r) => r.textContent?.includes("newer-call"))).toEqual([true, false]);
  });
});

describe("the qualified name", () => {
  it("joins namespace and table with one dot", () => {
    expect(qualifiedName({ namespace: "app_orders", table: "orders" })).toBe("app_orders.orders");
  });

  it("drops the dot when the table is empty", () => {
    expect(qualifiedName({ namespace: "automation:nightly", table: "" })).toBe(
      "automation:nightly"
    );
  });

  it("drops the dot when the namespace is empty", () => {
    expect(qualifiedName({ namespace: "", table: "orders" })).toBe("orders");
  });

  it("renders a table-less held write without a trailing dot", async () => {
    heldResult = {
      data: [
        entry({
          function: "automation",
          writes: [write({ plane: "automation", verb: "RUN", namespace: "nightly", table: "" })]
        })
      ],
      error: null,
      isLoading: false
    };
    render(<StagingBanner appId='app-1' />);
    await userEvent.click(screen.getByTestId("admin-app-staging-banner-count"));
    const [row] = await screen.findAllByTestId("admin-app-staging-held-row");
    expect(row.textContent?.endsWith("automation RUN nightly")).toBe(true);
  });
});

describe("the empty state", () => {
  it("says nothing held yet, naming what would show up here", async () => {
    heldResult = { data: [], error: null, isLoading: false };
    render(<StagingBanner appId='app-1' />);

    await userEvent.click(screen.getByTestId("admin-app-staging-banner-count"));

    expect((await screen.findByTestId("admin-app-staging-held-empty")).textContent).toBe(
      "Nothing held yet — writes staging can't isolate appear here."
    );
  });
});

describe("the 404 state", () => {
  it("says the caller can't open this app's staging, and hides the banner", () => {
    heldResult = {
      data: undefined,
      error: { response: { status: 404 } },
      isLoading: false
    };
    render(<StagingBanner appId='app-1' />);

    expect(screen.getByTestId("admin-app-staging-banner-forbidden").textContent).toBe(
      "You can't open this app's staging"
    );
    expect(screen.queryByTestId("admin-app-staging-banner")).toBeNull();
  });

  it("renders nothing yet for a non-404 error or the initial load", () => {
    heldResult = { data: undefined, error: new Error("network down"), isLoading: false };
    const { container: errContainer } = render(<StagingBanner appId='app-1' />);
    expect(errContainer.textContent).toBe("");

    cleanup();
    heldResult = { data: undefined, error: null, isLoading: true };
    const { container: loadingContainer } = render(<StagingBanner appId='app-1' />);
    expect(loadingContainer.textContent).toBe("");
  });
});

// ── Mounting ruling: draft + staging only ──────────────────────────────────

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

const renderPreview = (channel: "draft" | "published", draft: DraftTarget) =>
  render(
    <LivePreview
      app={app({ staging_url: draft.kind === "staging" ? draft.url : null })}
      device='desktop'
      channel={channel}
      draft={draft}
      nonce={1}
      path={null}
      onPathChange={() => undefined}
    />
  );

describe("where the banner is allowed to show", () => {
  it("shows over the draft frame once it resolved to the staging host", () => {
    renderPreview("draft", { kind: "staging", url: STAGING });
    expect(screen.getByTestId("admin-app-staging-banner")).toBeInTheDocument();
  });

  it("never shows over the Live/Published frame, even if a staging target is in hand", () => {
    // Contrived: `channel` is published, so LivePreview must ignore `draft`
    // entirely rather than trust a passed-in target. This is the ruling, not
    // a realistic prop combination a real caller would construct.
    renderPreview("published", { kind: "staging", url: STAGING });
    expect(screen.queryByTestId("admin-app-staging-banner")).toBeNull();
    expect(screen.queryByTestId("admin-app-staging-banner-forbidden")).toBeNull();
  });

  it("never shows when there is no staging host to frame", () => {
    renderPreview("draft", { kind: "unavailable", reason: "No staging host for this app." });
    expect(screen.queryByTestId("admin-app-staging-banner")).toBeNull();
    expect(screen.queryByTestId("admin-app-staging-banner-forbidden")).toBeNull();
  });

  it("never shows while the staging host is still pending", () => {
    renderPreview("draft", { kind: "pending" });
    expect(screen.queryByTestId("admin-app-staging-banner")).toBeNull();
  });
});
