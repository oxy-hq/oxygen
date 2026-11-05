// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { HiddenWidget, WidgetCard, WidgetPressResponse } from "@/types/homeWidgets";
import { HomeWidgets, needsCount } from "./index";

let widgets: WidgetCard[] | undefined;
let hidden: HiddenWidget[] = [];
let isError = false;
let isPending = false;
const refetch = vi.fn();
const widgetsEnabled = vi.fn();
vi.mock("@/hooks/api/homeWidgets/useHomeWidgets", () => ({
  useHomeWidgets: (_org: string, enabled?: boolean) => {
    widgetsEnabled(enabled);
    return { data: widgets, isError, isPending, refetch, isFetching: false };
  },
  useHiddenWidgets: () => hidden
}));
const preferences = { hide: vi.fn(), show: vi.fn(), setToday: vi.fn() };
vi.mock("@/hooks/api/homeWidgets/useHomePreferences", () => ({
  useHomePreferenceActions: () => preferences
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

const renderWidgets = (
  groups: { title: string | null; items: string[] }[] = [],
  variant: "rail" | "stack" = "rail"
) => render(<HomeWidgets orgSlug='poke-house' apps={[]} groups={groups} variant={variant} />);

afterEach(() => {
  cleanup();
  widgets = undefined;
  hidden = [];
  preferences.hide.mockReset();
  preferences.show.mockReset();
  preferences.setToday.mockReset();
  widgetsEnabled.mockReset();
  isError = false;
  isPending = false;
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

  it("draws a metric, and a full-size card with every row", () => {
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
    expect(screen.getByTestId("home-widget-bookkeeping-close")).toHaveAttribute(
      "data-size",
      "full"
    );
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
    renderWidgets([
      { title: "Stores", items: ["bookkeeping/close#valencia", "bookkeeping/close#mission"] }
    ]);
    expect(screen.getByRole("heading", { name: "Stores" })).toBeInTheDocument();
    const cards = screen.getAllByTestId(/^home-widget-bookkeeping-close-(mission|valencia)$/);
    expect(cards.map((c) => c.getAttribute("data-testid"))).toEqual([
      "home-widget-bookkeeping-close-valencia",
      "home-widget-bookkeeping-close-mission"
    ]);
    expect(cards[0]).toHaveTextContent("Bookkeeping · Valencia");
    expect(screen.queryByTestId("home-widget-bookkeeping-close")).toBeNull();
  });

  it("folds a widget that failed to answer into a quiet Couldn't load line, its reason on hover", () => {
    widgets = [close({ view: null, error: "This widget took too long to answer." })];
    renderWidgets();
    const line = screen.getByTestId("home-widget-bookkeeping-close");
    expect(line).toHaveAttribute("data-compact", "true");
    expect(line).toHaveAttribute("title", "This widget took too long to answer.");
    expect(screen.getByTestId("home-widget-bookkeeping-close-headline")).toHaveTextContent(
      "Couldn't load"
    );
    expect(screen.getByTestId("home-widgets-failed")).toHaveTextContent("Couldn't load");
  });

  it("shows Today's shape while the widgets load", () => {
    widgets = undefined;
    isPending = true;
    renderWidgets();
    expect(screen.getByTestId("home-widgets-loading")).toHaveAttribute("aria-busy", "true");
  });

  it("draws progress as a ring holding done/total, and caps a regular card at three rows", () => {
    const rows = Array.from({ length: 5 }, (_, i) => ({
      title: `Store ${i}`,
      detail: null,
      state: "ready" as const,
      href: null,
      button: null
    }));
    widgets = [close({ size: "regular", view: { ...close().view!, rows } })];
    renderWidgets();
    const ring = screen.getByTestId("home-widget-bookkeeping-close-progress");
    expect(ring).toHaveAttribute("aria-valuenow", "61");
    expect(ring).toHaveTextContent("61/84");
    expect(screen.getByTestId("home-widget-bookkeeping-close-row-2")).toBeInTheDocument();
    expect(screen.queryByTestId("home-widget-bookkeeping-close-row-3")).toBeNull();
    expect(screen.getByTestId("home-widget-bookkeeping-close-more")).toHaveTextContent(
      "2 more in Bookkeeping"
    );
  });

  it("keeps every row of a card whose rows carry buttons, so no button is cut", () => {
    const rows = Array.from({ length: 5 }, (_, i) => ({
      title: `Store ${i}`,
      detail: null,
      state: "ready" as const,
      href: null,
      button: { ...post, key: `s${i}` }
    }));
    widgets = [close({ size: "regular", view: { ...close().view!, rows } })];
    renderWidgets();
    expect(screen.getByTestId("home-widget-bookkeeping-close-row-4-button")).toBeInTheDocument();
    expect(screen.queryByTestId("home-widget-bookkeeping-close-more")).toBeNull();
  });

  it("names each card's Open link after its app and widget", () => {
    widgets = [close()];
    renderWidgets();
    expect(screen.getByTestId("home-widget-bookkeeping-close-open")).toHaveAccessibleName(
      "Open Bookkeeping: Month-end close"
    );
  });

  it("puts every card that needs someone first, then every calm widget in one All clear list", () => {
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
    // The calm one leads the first group and the card with a job sits in the second.
    renderWidgets([
      { title: "Stores", items: ["bookkeeping/close#first"] },
      { title: "Back office", items: ["bookkeeping/close", "bookkeeping/close#last"] }
    ]);
    const strip = screen.getByTestId("home-widget-bookkeeping-close-first");
    expect(strip).toHaveAttribute("data-compact", "true");
    expect(strip).toHaveAttribute(
      "href",
      "https://poke-house--bookkeeping.apps.test/?month=2026-09"
    );
    // A group with only calm widgets has no heading of its own; they are under All clear.
    expect(screen.queryByRole("heading", { name: "Stores" })).toBeNull();
    expect(screen.getByRole("heading", { name: "Back office" })).toBeInTheDocument();
    const calmBlock = screen.getByTestId("home-widgets-calm");
    expect(calmBlock).toHaveTextContent("All clear");
    const order = screen
      .getAllByTestId(/^home-widget-bookkeeping-close(-first|-last)?$/)
      .map((el) => el.getAttribute("data-testid"));
    expect(order).toEqual([
      "home-widget-bookkeeping-close",
      "home-widget-bookkeeping-close-first",
      "home-widget-bookkeeping-close-last"
    ]);
  });

  it("leaves out the All clear heading when every widget is calm", () => {
    const calm = close({
      view: { ...close().view!, level: "ok", buttons: [], rows: [], progress: null }
    });
    widgets = [calm];
    renderWidgets();
    expect(screen.getByTestId("home-widgets-calm")).not.toHaveTextContent("All clear");
    expect(screen.getByTestId("home-widget-bookkeeping-close")).toHaveAttribute(
      "data-compact",
      "true"
    );
  });

  it("marks a widget that changed since the last visit, and one the home did not show then", () => {
    const calm = close({
      key: "calm",
      id: "calm",
      view: { ...close().view!, level: "ok", buttons: [], rows: [], progress: null }
    });
    widgets = [close(), calm];
    render(
      <HomeWidgets
        orgSlug='poke-house'
        apps={[]}
        groups={[]}
        variant='rail'
        since={
          new Map([
            [close().key, "updated"],
            ["calm", "new"]
          ])
        }
      />
    );
    expect(screen.getByTestId("home-widget-bookkeeping-close-since")).toHaveTextContent("Updated");
    expect(screen.getByTestId("home-widget-bookkeeping-calm-since")).toHaveTextContent("New");
  });

  it("stacks in the same order under the apps on a narrow screen", () => {
    widgets = [close()];
    renderWidgets([], "stack");
    expect(screen.getByTestId("home-widgets")).toHaveAttribute("data-variant", "stack");
    expect(screen.getByTestId("home-widget-bookkeeping-close").tagName).toBe("ARTICLE");
  });

  it("keeps a calm widget whole when it offers a button", () => {
    widgets = [close({ view: { ...close().view!, level: "ok" } })];
    renderWidgets();
    expect(screen.getByTestId("home-widget-bookkeeping-close").tagName).toBe("ARTICLE");
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
    rerender(<HomeWidgets orgSlug='poke-house' apps={[]} groups={[]} variant='rail' />);
    expect(screen.getByTestId("home-widget-bookkeeping-close").tagName).toBe("ARTICLE");
    expect(screen.getByTestId("home-widget-bookkeeping-close-message")).toHaveTextContent(
      "Posted 19 journal entries"
    );
  });

  it("counts for the status line only the widgets that need someone", () => {
    const calm = close({ view: { ...close().view!, level: "ok", buttons: [], rows: [] } });
    // A widget that failed to answer is its app owner's news, not a job for the reader.
    expect(needsCount([close(), calm, close({ view: null, error: "Timed out." })])).toBe(1);
    expect(needsCount([calm])).toBe(0);
    expect(needsCount(undefined)).toBe(0);
  });

  it("offers a retry when the widgets could not load", () => {
    isError = true;
    renderWidgets();
    fireEvent.click(screen.getByTestId("home-widgets-retry"));
    expect(refetch).toHaveBeenCalled();
  });
  describe("hiding what the person does not want", () => {
    const baseView = close().view as NonNullable<WidgetCard["view"]>;
    const calmCopy = (instance: string): WidgetCard =>
      close({
        key: `bookkeeping/close#${instance}`,
        instance,
        title: instance,
        view: { ...baseView, level: "ok", buttons: [], rows: [], progress: null }
      });

    it("hides a card from its More menu once its exit has played", async () => {
      widgets = [close()];
      renderWidgets();
      await userEvent.click(screen.getByTestId("home-widget-bookkeeping-close-menu"));
      await userEvent.click(await screen.findByTestId("home-widget-hide"));
      const card = screen.getByTestId("home-widget-bookkeeping-close");
      // It starts to leave at once, and is hidden for real when the motion ends.
      expect(card).toHaveAttribute("data-leaving", "true");
      expect(preferences.hide).not.toHaveBeenCalled();
      fireEvent.transitionEnd(card, { propertyName: "opacity" });
      expect(preferences.hide).toHaveBeenCalledWith(
        expect.objectContaining({ key: "bookkeeping/close" })
      );
      expect(preferences.hide).toHaveBeenCalledTimes(1);
    });

    it("hides a folded line from its menu, which is a button beside the link, not inside it", async () => {
      widgets = [calmCopy("first")];
      renderWidgets();
      const line = screen.getByTestId("home-widget-bookkeeping-close-first");
      const menu = screen.getByTestId("home-widget-bookkeeping-close-first-menu");
      expect(line.contains(menu)).toBe(false);
      await userEvent.click(menu);
      await userEvent.click(await screen.findByTestId("home-widget-hide"));
      fireEvent.transitionEnd(screen.getByTestId("home-widget-bookkeeping-close-first-line"), {
        propertyName: "opacity"
      });
      expect(preferences.hide).toHaveBeenCalledWith(
        expect.objectContaining({ key: "bookkeeping/close#first" })
      );
    });

    it("keeps the menu reachable without a pointer: transparent until hover or focus, never removed", () => {
      widgets = [close()];
      renderWidgets();
      const menu = screen.getByTestId("home-widget-bookkeeping-close-menu");
      expect(menu).toHaveAccessibleName("More: Bookkeeping: Month-end close");
      expect(menu.className).toContain("group-focus-within/widget:opacity-100");
      expect(menu.className).toContain("pointer-coarse:opacity-100");
      expect(menu).not.toHaveAttribute("hidden");
    });

    it("says how many are hidden, and lists them with Show", async () => {
      widgets = [close()];
      hidden = [
        { key: "bookkeeping/payroll", title: "Payroll", app: close().app },
        { key: "bookkeeping/tax", title: "Tax", app: close().app }
      ];
      renderWidgets();
      const line = screen.getByTestId("home-widgets-hidden");
      expect(line).toHaveTextContent("2 hidden");
      await userEvent.click(line);
      const row = await screen.findByTestId("home-widget-hidden-bookkeeping/payroll");
      expect(row).toHaveTextContent("Payroll");
      await userEvent.click(within(row).getByTestId("home-widget-show"));
      expect(preferences.show).toHaveBeenCalledWith("bookkeeping/payroll");
    });

    it("has no hidden line while nothing is hidden", () => {
      widgets = [close()];
      renderWidgets();
      expect(screen.queryByTestId("home-widgets-hidden")).toBeNull();
    });

    it("keeps Today and the way back when every card is hidden", () => {
      widgets = [];
      hidden = [{ key: "bookkeeping/close", title: "Month-end close", app: close().app }];
      renderWidgets();
      expect(screen.getByTestId("home-widgets")).toBeInTheDocument();
      expect(screen.getByTestId("home-widgets-hidden")).toHaveTextContent("1 hidden");
    });

    it("hides Today from its heading", async () => {
      widgets = [close()];
      renderWidgets();
      const hide = screen.getByTestId("home-today-hide");
      expect(hide).toHaveAccessibleName("Hide Today");
      await userEvent.click(hide);
      expect(preferences.setToday).toHaveBeenCalledWith("hidden");
    });

    it("still hides Today inside a fold, where it has no heading of its own", async () => {
      widgets = [close()];
      render(
        <HomeWidgets orgSlug='poke-house' apps={[]} groups={[]} variant='stack' heading={false} />
      );
      expect(screen.queryByRole("heading", { name: "Today" })).toBeNull();
      expect(screen.getByTestId("home-widgets")).toHaveAccessibleName("Today");
      await userEvent.click(screen.getByTestId("home-today-hide"));
      expect(preferences.setToday).toHaveBeenCalledWith("hidden");
    });

    it("reads nothing while told not to, and shows the skeleton meanwhile", () => {
      widgets = undefined;
      isPending = true;
      render(
        <HomeWidgets orgSlug='poke-house' apps={[]} groups={[]} variant='rail' enabled={false} />
      );
      expect(widgetsEnabled).toHaveBeenCalledWith(false);
      expect(screen.getByTestId("home-widgets-loading")).toBeInTheDocument();
    });
  });
});
