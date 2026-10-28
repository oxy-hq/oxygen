// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CustomAppSummary } from "@/types/apps";
import { AppCard } from "./AppCard";

afterEach(cleanup);

beforeEach(() => {
  document.head.innerHTML = "";
});

const app = (over: Partial<CustomAppSummary> = {}): CustomAppSummary => ({
  id: "app-1",
  slug: "revenue",
  name: "Revenue",
  org_slug: "acme",
  url: "/customer-apps/acme/revenue/",
  published_at: "2026-07-30T00:00:00Z",
  visibility: "org",
  ...over
});

describe("AppCard access control", () => {
  // The whole point of the prop: the launcher passes it only for org owners and
  // admins. A member's card must not mention access at all — not the button, and
  // not the badge that would tell them which apps are locked down.
  it("shows nothing about access without onManageAccess", () => {
    render(<AppCard app={app({ visibility: "members" })} />);
    expect(screen.queryByRole("button", { name: /access/i })).toBeNull();
    expect(screen.queryByText(/restricted/i)).toBeNull();
  });

  it("offers the control to a manager", async () => {
    const onManageAccess = vi.fn();
    render(<AppCard app={app()} onManageAccess={onManageAccess} />);
    await userEvent.click(screen.getByRole("button", { name: /access/i }));
    expect(onManageAccess).toHaveBeenCalledWith(expect.objectContaining({ id: "app-1" }));
  });

  it("badges only the restricted app, and only for a manager", () => {
    const { rerender } = render(<AppCard app={app()} onManageAccess={vi.fn()} />);
    // An open app is the default and true of most cards, so it gets no chip —
    // otherwise every tile carries a badge that says nothing.
    expect(screen.queryByText(/restricted/i)).toBeNull();

    rerender(<AppCard app={app({ visibility: "members" })} onManageAccess={vi.fn()} />);
    expect(screen.getByText(/restricted/i)).toBeInTheDocument();
  });

  it("keeps the card a link to the app", () => {
    render(<AppCard app={app()} onManageAccess={vi.fn()} />);
    // The name's link stretches over the card. Adding a second action must not
    // cost the card its one-click navigation.
    expect(screen.getByRole("link", { name: /^Revenue/ })).toHaveAttribute(
      "href",
      "/customer-apps/acme/revenue/"
    );
  });
});

describe("AppCard opening behaviour", () => {
  /// The app opens in this tab, never inside HQ: it is a document of its own,
  /// so it owns the address bar (its query-string state is linkable, a reload
  /// reloads the app), and Back returns to HQ. The app draws the same rail, so
  /// the next app is one click away there. apps-first-shell.md § 4.1.
  it("opens the app in this tab", () => {
    render(<AppCard app={app()} />);
    const link = screen.getByRole("link", { name: "Revenue" });
    expect(link).toHaveAttribute("href", app().url);
    expect(link).not.toHaveAttribute("target");
  });

  /// Nothing intercepts the click. The card is a plain anchor, so cmd-click,
  /// middle-click, "open in new tab" and "copy link address" are all the
  /// browser's — re-implementing any of them is how in-app routers break them.
  it("leaves the click to the browser", () => {
    render(<AppCard app={app()} />);
    const link = screen.getByRole("link", { name: /^Revenue/ });
    const clicked = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(clicked);
    expect(clicked.defaultPrevented).toBe(false);
  });

  /// Hover warms the app's HTML — which also pulls its entry chunks via the
  /// response's preload hints — so the click lands on something already parsed.
  /// A `rel=prefetch` link rather than a `fetch()`, so the serve path can tell
  /// a hover from an open and not record a view for it.
  it("prefetches the app on hover and on keyboard focus", async () => {
    render(<AppCard app={app()} />);
    const prefetches = () =>
      document.head.querySelectorAll('link[rel="prefetch"][href="/customer-apps/acme/revenue/"]');

    await userEvent.hover(screen.getByRole("link", { name: /^Revenue/ }));
    expect(prefetches()).toHaveLength(1);

    // Reaching the card by Tab has to be as fast as reaching it by mouse — and
    // must not queue a second link for the same URL.
    screen.getByRole("link", { name: /^Revenue/ }).focus();
    expect(prefetches()).toHaveLength(1);
  });
});

describe("AppCard art", () => {
  /// The launcher marks only its first row eager. Every other card stays lazy, so
  /// a long grid doesn't download images nobody has scrolled to.
  it("lazy-loads the art unless asked to load it eagerly", () => {
    const withArt = app({ art_url: "/customer-apps/acme/revenue/card.webp" });
    const { container, rerender } = render(<AppCard app={withArt} />);
    const art = () => container.querySelector('img[src="/customer-apps/acme/revenue/card.webp"]');
    expect(art()).toHaveAttribute("loading", "lazy");
    expect(art()).toHaveAttribute("decoding", "async");

    rerender(<AppCard app={withArt} eagerArt />);
    expect(art()).toHaveAttribute("loading", "eager");
  });
});
