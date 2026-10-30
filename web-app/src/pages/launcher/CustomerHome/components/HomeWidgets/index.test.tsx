// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { WidgetCard, WidgetPressResponse } from "@/types/homeWidgets";
import { HomeWidgets } from "./index";

let widgets: WidgetCard[] | undefined;
let isError = false;
const refetch = vi.fn();
vi.mock("@/hooks/api/homeWidgets/useHomeWidgets", () => ({
  useHomeWidgets: () => ({ data: widgets, isError, refetch, isFetching: false })
}));

type Callbacks = {
  onSuccess?: (a: WidgetPressResponse) => void;
  onError?: (e: unknown) => void;
  onSettled?: () => void;
};
const mutate = vi.fn();
let answer: { ok: WidgetPressResponse } | { error: unknown } = {
  ok: { ok: true, message: null, view: null }
};
vi.mock("@/hooks/api/homeWidgets/usePressWidget", () => ({
  usePressWidget: () => ({
    isPending: false,
    mutate: (press: unknown, cb: Callbacks) => {
      mutate(press);
      if ("ok" in answer) cb.onSuccess?.(answer.ok);
      else cb.onError?.(answer.error);
      cb.onSettled?.();
    }
  })
}));

const post = {
  action: "post-ready",
  label: "Post 19 ready",
  input: { period: "2026-09" },
  key: "2026-09",
  confirm: "Post 19 journal entries to QuickBooks?",
  destructive: false
};

const close = (over: Partial<WidgetCard> = {}): WidgetCard => ({
  app: {
    id: "app-1",
    slug: "bookkeeping",
    name: "Bookkeeping",
    org_slug: "poke-house",
    url: "https://poke-house--bookkeeping.apps.test/"
  },
  key: "bookkeeping/close",
  id: "close",
  instance: null,
  title: "Month-end close",
  params: {},
  size: "wide",
  sizes: ["regular", "wide", "full"],
  view: {
    headline: "19 stores ready to post",
    detail: "September · 61 of 84 posted",
    level: "attention",
    progress: { done: 61, total: 84 },
    metric: null,
    rows: [
      {
        title: "Valencia",
        detail: "Not mapped to a QuickBooks location",
        state: "blocked",
        href: "https://poke-house--bookkeeping.apps.test/settings",
        button: null
      }
    ],
    buttons: [post],
    href: "https://poke-house--bookkeeping.apps.test/?month=2026-09",
    badge: 19,
    refresh_after: null
  },
  error: null,
  ...over
});

const renderWidgets = (group?: { title: string | null; items: string[] }) =>
  render(<HomeWidgets orgSlug='poke-house' apps={[]} group={group} />);

afterEach(() => {
  cleanup();
  widgets = undefined;
  isError = false;
  mutate.mockReset();
  answer = { ok: { ok: true, message: null, view: null } };
});

describe("the home's widgets", () => {
  it("draws nothing while no app has a widget", () => {
    widgets = [];
    renderWidgets();
    expect(screen.queryByTestId("home-widgets")).toBeNull();
  });

  it("draws an app's view: headline, progress, rows, and Open into the app", () => {
    widgets = [close()];
    renderWidgets();
    const base = "home-widget-bookkeeping-close";
    expect(screen.getByTestId(`${base}-headline`)).toHaveTextContent("19 stores ready to post");
    expect(screen.getByTestId(base)).toHaveAttribute("data-level", "attention");
    expect(screen.getByTestId(`${base}-progress`)).toHaveAttribute("aria-valuenow", "61");
    expect(screen.getByTestId(`${base}-row-0`)).toHaveTextContent("Blocked");
    expect(screen.getByTestId(`${base}-open`)).toHaveAttribute(
      "href",
      "https://poke-house--bookkeeping.apps.test/?month=2026-09"
    );
  });

  it("asks before running a button that must confirm, then sends its input and key", () => {
    widgets = [close()];
    answer = { ok: { ok: true, message: "Posted 19 journal entries", view: null } };
    renderWidgets();
    const id = "home-widget-bookkeeping-close-button-post-ready";
    fireEvent.click(screen.getByTestId(id));
    expect(mutate).not.toHaveBeenCalled();
    expect(screen.getByTestId(`${id}-confirm`)).toHaveTextContent(
      "Post 19 journal entries to QuickBooks?"
    );
    fireEvent.click(screen.getByTestId(`${id}-yes`));
    expect(mutate).toHaveBeenCalledWith({
      appId: "app-1",
      widget: "close",
      key: "bookkeeping/close",
      button: post
    });
    expect(screen.getByTestId("home-widget-bookkeeping-close-message")).toHaveTextContent(
      "Posted 19 journal entries"
    );
  });

  it("runs a button with no confirmation at once, and shows why a press failed", () => {
    widgets = [close({ view: { ...close().view!, buttons: [{ ...post, confirm: null }] } })];
    answer = {
      error: Object.assign(new Error("x"), {
        isAxiosError: true,
        response: { data: { message: "Valencia isn't mapped" } }
      })
    };
    renderWidgets();
    fireEvent.click(screen.getByTestId("home-widget-bookkeeping-close-button-post-ready"));
    expect(mutate).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("home-widget-bookkeeping-close-error")).toHaveTextContent(
      "Valencia isn't mapped"
    );
  });

  it("spans the row at full size and draws a metric", () => {
    widgets = [
      close({
        size: "full",
        view: {
          ...close().view!,
          metric: {
            value: "$48,210",
            label: "Posted this month",
            delta: "+12%",
            trend: "up",
            spark: [1, 3, 2, 5]
          }
        }
      })
    ];
    renderWidgets();
    const card = screen.getByTestId("home-widget-bookkeeping-close");
    expect(card).toHaveAttribute("data-size", "full");
    expect(card.className).toContain("lg:col-span-3");
    expect(screen.getByTestId("home-widget-bookkeeping-close-metric")).toHaveTextContent(
      "$48,210Posted this month+12%"
    );
  });

  it("draws a titled group's copies in its order, two copies of one widget apart", () => {
    const copy = (instance: string, title: string): WidgetCard =>
      close({
        key: `bookkeeping/close#${instance}`,
        instance,
        title,
        params: { store: instance }
      });
    widgets = [close(), copy("mission", "Mission St"), copy("valencia", "Valencia")];
    renderWidgets({
      title: "Stores",
      items: ["bookkeeping/close#valencia", "bookkeeping/close#mission"]
    });
    expect(screen.getByRole("heading", { name: "Stores" })).toBeInTheDocument();
    const cards = screen.getAllByTestId(/^home-widget-bookkeeping-close-(mission|valencia)$/);
    expect(cards.map((c) => c.getAttribute("data-testid"))).toEqual([
      "home-widget-bookkeeping-close-valencia",
      "home-widget-bookkeeping-close-mission"
    ]);
    expect(cards[0]).toHaveTextContent("Bookkeeping · Valencia");
    expect(screen.queryByTestId("home-widget-bookkeeping-close")).toBeNull();
  });

  it("says why a widget has no view", () => {
    widgets = [close({ view: null, error: "This widget took too long to answer." })];
    renderWidgets();
    expect(screen.getByTestId("home-widget-bookkeeping-close-error")).toHaveTextContent(
      "This widget took too long to answer."
    );
  });

  it("draws progress as a ring beside the headline on a one-column card, a bar on a wider one", () => {
    widgets = [
      close({ size: "regular" }),
      close({ key: "bookkeeping/close#wide", instance: "wide", size: "wide" })
    ];
    renderWidgets();
    const ring = screen.getByTestId("home-widget-bookkeeping-close-progress");
    expect(ring).toHaveAttribute("data-variant", "ring");
    expect(ring).toHaveAttribute("aria-valuenow", "61");
    expect(ring).toHaveTextContent("61/84");
    expect(screen.getByTestId("home-widget-bookkeeping-close-wide-progress")).toHaveAttribute(
      "data-variant",
      "bar"
    );
  });

  it("folds a calm widget to one line after the cards with something to do", () => {
    const calm = (instance: string, level: "ok" | "info"): WidgetCard =>
      close({
        key: `bookkeeping/close#${instance}`,
        instance,
        title: instance,
        view: {
          ...close().view!,
          headline: "September is closed",
          level,
          buttons: [],
          rows: [],
          progress: null
        }
      });
    widgets = [calm("first", "ok"), close(), calm("last", "info")];
    renderWidgets({
      title: null,
      items: ["bookkeeping/close#first", "bookkeeping/close", "bookkeeping/close#last"]
    });
    const strip = screen.getByTestId("home-widget-bookkeeping-close-first");
    expect(strip).toHaveAttribute("data-compact", "true");
    expect(strip).toHaveAttribute(
      "href",
      "https://poke-house--bookkeeping.apps.test/?month=2026-09"
    );
    expect(screen.getByTestId("home-widget-bookkeeping-close-first-headline")).toHaveTextContent(
      "September is closed"
    );
    // The card with a job keeps its size and comes first; the calm two follow in the group's order.
    expect(screen.getByTestId("home-widget-bookkeeping-close").tagName).toBe("ARTICLE");
    const order = screen
      .getAllByTestId(/^home-widget-bookkeeping-close(-first|-last)?$/)
      .map((el) => el.getAttribute("data-testid"));
    expect(order).toEqual([
      "home-widget-bookkeeping-close",
      "home-widget-bookkeeping-close-first",
      "home-widget-bookkeeping-close-last"
    ]);
  });

  it("keeps a calm widget whole when it offers a button, and a failed one too", () => {
    widgets = [
      close({ view: { ...close().view!, level: "ok" } }),
      close({
        key: "bookkeeping/close#broken",
        instance: "broken",
        view: null,
        error: "Timed out."
      })
    ];
    renderWidgets();
    expect(screen.getByTestId("home-widget-bookkeeping-close").tagName).toBe("ARTICLE");
    expect(screen.getByTestId("home-widget-bookkeeping-close-broken").tagName).toBe("ARTICLE");
    expect(screen.queryByTestId("home-widgets-calm")).toBeNull();
  });

  it("keeps a pressed card whole when its answer turns it calm, so the answer stays in sight", () => {
    widgets = [close({ view: { ...close().view!, buttons: [{ ...post, confirm: null }] } })];
    answer = { ok: { ok: true, message: "Posted 19 journal entries", view: null } };
    const { rerender } = renderWidgets();
    fireEvent.click(screen.getByTestId("home-widget-bookkeeping-close-button-post-ready"));
    widgets = [
      close({
        view: {
          ...close().view!,
          headline: "September is closed",
          level: "ok",
          buttons: [],
          rows: []
        }
      })
    ];
    rerender(<HomeWidgets orgSlug='poke-house' apps={[]} />);
    expect(screen.getByTestId("home-widget-bookkeeping-close").tagName).toBe("ARTICLE");
    expect(screen.getByTestId("home-widget-bookkeeping-close-message")).toHaveTextContent(
      "Posted 19 journal entries"
    );
  });

  it("offers a retry when the widgets could not load", () => {
    isError = true;
    renderWidgets();
    fireEvent.click(screen.getByTestId("home-widgets-retry"));
    expect(refetch).toHaveBeenCalled();
  });
});
