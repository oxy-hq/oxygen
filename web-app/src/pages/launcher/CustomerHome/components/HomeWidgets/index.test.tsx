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
  id: "close",
  title: "Month-end close",
  size: "wide",
  view: {
    headline: "19 stores ready to post",
    detail: "September · 61 of 84 posted",
    level: "attention",
    progress: { done: 61, total: 84 },
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
    badge: 19
  },
  error: null,
  ...over
});

const renderWidgets = () => render(<HomeWidgets orgSlug='poke-house' apps={[]} />);

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
    expect(mutate).toHaveBeenCalledWith({ appId: "app-1", widget: "close", button: post });
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

  it("says why a widget has no view", () => {
    widgets = [close({ view: null, error: "This widget took too long to answer." })];
    renderWidgets();
    expect(screen.getByTestId("home-widget-bookkeeping-close-error")).toHaveTextContent(
      "This widget took too long to answer."
    );
  });

  it("offers a retry when the widgets could not load", () => {
    isError = true;
    renderWidgets();
    fireEvent.click(screen.getByTestId("home-widgets-retry"));
    expect(refetch).toHaveBeenCalled();
  });
});
