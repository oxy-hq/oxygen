// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  HiddenWidget,
  HomePreferences,
  HomeWidgetsResponse,
  WidgetCard
} from "@/types/homeWidgets";
import queryKeys from "../queryKey";
import { applyPreferences, restores, sectionsFromKinds } from "./preferences";
import { useHomePreferenceActions, useHomePreferences } from "./useHomePreferences";
import { useHomeWidgets } from "./useHomeWidgets";

const service = vi.hoisted(() => ({
  list: vi.fn(),
  preferences: vi.fn(),
  savePreferences: vi.fn()
}));
vi.mock("@/services/api/homeWidgets", () => ({ HomeWidgetsService: service }));
const toastError = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { error: toastError } }));

const app = {
  id: "app-1",
  slug: "bookkeeping",
  name: "Bookkeeping",
  org_slug: "poke-house",
  url: "https://poke-house--bookkeeping.apps.test/"
};
const card = (id: string): WidgetCard => ({
  app,
  key: `bookkeeping/${id}`,
  id,
  instance: null,
  title: id,
  params: {},
  size: "regular",
  sizes: ["regular"],
  view: null,
  error: "x"
});
const response = (over: Partial<HomeWidgetsResponse> = {}): HomeWidgetsResponse => ({
  widgets: [card("close"), card("payroll")],
  sections: [
    { kind: "widgets", title: null, items: ["bookkeeping/close", "bookkeeping/payroll"] },
    { kind: "apps" }
  ],
  hidden: [],
  today: "shown",
  ...over
});

const ORG = "poke-house";
const prefsKey = queryKeys.homePreferences.get(ORG);
const widgetsKey = queryKeys.homeWidgets.list(ORG);

let qc: QueryClient;
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={qc}>{children}</QueryClientProvider>
);
const setup = (initial: HomeWidgetsResponse = response(), prefs?: HomePreferences) => {
  qc.setQueryData(widgetsKey, initial);
  qc.setQueryData(prefsKey, prefs ?? { hidden: [], today: "shown" });
  // Someone is looking at the widgets, as the home is: a refetch would show.
  renderHook(() => useHomeWidgets(ORG), { wrapper });
  return renderHook(() => useHomePreferenceActions(ORG), { wrapper }).result;
};
const shown = () => qc.getQueryData<HomeWidgetsResponse>(widgetsKey)?.widgets.map((w) => w.key);

beforeEach(() => {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  service.list.mockResolvedValue(response());
  service.preferences.mockResolvedValue({ hidden: [], today: "shown" });
  service.savePreferences.mockImplementation((_org: string, next: HomePreferences) =>
    Promise.resolve(next)
  );
});
afterEach(() => {
  vi.clearAllMocks();
});

describe("applyPreferences", () => {
  it("moves a hidden copy out of the widgets and their group's items, naming it in hidden", () => {
    const next = applyPreferences(response(), { hidden: ["bookkeeping/close"], today: "shown" });
    expect(next.widgets.map((w) => w.key)).toEqual(["bookkeeping/payroll"]);
    expect(next.sections[0]).toEqual({
      kind: "widgets",
      title: null,
      items: ["bookkeeping/payroll"]
    });
    expect(next.hidden).toEqual([{ key: "bookkeeping/close", title: "close", app }]);
  });

  it("drops a restored copy from hidden and echoes Today", () => {
    const was: HiddenWidget = { key: "bookkeeping/tax", title: "Tax", app };
    const next = applyPreferences(response({ hidden: [was] }), { hidden: [], today: "hidden" });
    expect(next.hidden).toEqual([]);
    expect(next.today).toBe("hidden");
  });

  it("knows when a change brings a copy back", () => {
    const hidden: HomePreferences = { hidden: ["a"], today: "shown" };
    expect(restores(hidden, { hidden: [], today: "shown" })).toBe(true);
    expect(restores(hidden, { hidden: ["a", "b"], today: "shown" })).toBe(false);
  });
});

describe("sectionsFromKinds", () => {
  it("rebuilds the layout's sections from their kinds, and names none when there are none", () => {
    expect(sectionsFromKinds(["apps", "widgets", "insights"])).toEqual([
      { kind: "apps" },
      { kind: "widgets", title: null, items: [] },
      { kind: "insights" }
    ]);
    expect(sectionsFromKinds(["apps"])).toEqual([{ kind: "apps" }]);
    expect(sectionsFromKinds([])).toBeUndefined();
    expect(sectionsFromKinds(undefined)).toBeUndefined();
  });
});

describe("useHomePreferenceActions", () => {
  it("hides a card at once and PUTs the whole set", async () => {
    const result = setup();
    act(() => result.current.hide(card("close")));
    await waitFor(() => expect(shown()).toEqual(["bookkeeping/payroll"]));
    expect(qc.getQueryData(prefsKey)).toEqual({ hidden: ["bookkeeping/close"], today: "shown" });
    await waitFor(() =>
      expect(service.savePreferences).toHaveBeenCalledWith(ORG, {
        hidden: ["bookkeeping/close"],
        today: "shown"
      })
    );
    // Hiding reads no widget again: nothing has to run.
    expect(service.list).not.toHaveBeenCalled();
  });

  it("keeps both of two quick hides", async () => {
    const result = setup();
    act(() => {
      result.current.hide(card("close"));
      result.current.hide(card("payroll"));
    });
    await waitFor(() => expect(service.savePreferences).toHaveBeenCalledTimes(2));
    expect(service.savePreferences).toHaveBeenLastCalledWith(ORG, {
      hidden: ["bookkeeping/close", "bookkeeping/payroll"],
      today: "shown"
    });
  });

  it("puts the card back and says so when the server refuses", async () => {
    service.savePreferences.mockRejectedValue(new Error("422"));
    const result = setup();
    act(() => result.current.hide(card("close")));
    await waitFor(() => expect(toastError).toHaveBeenCalledWith("Couldn't save that. Try again."));
    expect(shown()).toEqual(["bookkeeping/close", "bookkeeping/payroll"]);
    expect(qc.getQueryData(prefsKey)).toEqual({ hidden: [], today: "shown" });
  });

  it("shows a copy again by reading the widgets again, so it can run", async () => {
    const hidden: HiddenWidget = { key: "bookkeeping/tax", title: "Tax", app };
    const result = setup(response({ hidden: [hidden] }), {
      hidden: ["bookkeeping/tax"],
      today: "shown"
    });
    act(() => result.current.show("bookkeeping/tax"));
    await waitFor(() =>
      expect(service.savePreferences).toHaveBeenCalledWith(ORG, { hidden: [], today: "shown" })
    );
    expect(qc.getQueryData<HomeWidgetsResponse>(widgetsKey)?.hidden).toEqual([]);
    await waitFor(() => expect(service.list).toHaveBeenCalled());
  });

  it("hides and shows Today, keeping the hidden copies", async () => {
    const result = setup(response(), { hidden: ["bookkeeping/close"], today: "shown" });
    act(() => result.current.setToday("hidden"));
    await waitFor(() =>
      expect(service.savePreferences).toHaveBeenCalledWith(ORG, {
        hidden: ["bookkeeping/close"],
        today: "hidden"
      })
    );
    expect(qc.getQueryData<HomeWidgetsResponse>(widgetsKey)?.today).toBe("hidden");
  });

  it("sends the choices alone and keeps the layout's sections in the cache", async () => {
    const sections = ["apps", "insights"] as const;
    const result = setup(response(), { hidden: [], today: "shown", sections: [...sections] });
    act(() => result.current.setToday("hidden"));
    await waitFor(() => expect(service.savePreferences).toHaveBeenCalledTimes(1));
    // The body is `{ hidden, today }` only, though the cache also holds `sections`.
    expect(service.savePreferences.mock.calls[0][1]).toStrictEqual({
      hidden: [],
      today: "hidden"
    });
    expect(qc.getQueryData(prefsKey)).toEqual({
      hidden: [],
      today: "hidden",
      sections: [...sections]
    });
  });

  it("reads the widgets again when an earlier save of two quick hides was refused", async () => {
    // The server ends up with both hidden: the first PUT fails, the second's set holds both.
    const close = card("close");
    const payroll = card("payroll");
    const named = (c: WidgetCard): HiddenWidget => ({ key: c.key, title: c.title, app });
    const server = response({
      widgets: [],
      sections: [{ kind: "widgets", title: null, items: [] }, { kind: "apps" }],
      hidden: [named(close), named(payroll)]
    });
    const serverPrefs: HomePreferences = {
      hidden: [close.key, payroll.key],
      today: "shown"
    };
    service.list.mockResolvedValue(server);
    service.preferences.mockResolvedValue(serverPrefs);
    let refuseFirst: (e: Error) => void = () => {};
    let landSecond: (p: HomePreferences) => void = () => {};
    service.savePreferences
      .mockImplementationOnce(
        () =>
          new Promise((_, reject) => {
            refuseFirst = reject;
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            landSecond = resolve;
          })
      );
    const result = setup();
    renderHook(() => useHomePreferences(ORG), { wrapper });

    act(() => {
      result.current.hide(close);
      result.current.hide(payroll);
    });
    await waitFor(() => expect(service.savePreferences).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(shown()).toEqual([]));

    // The first is refused while the second is in flight: its rollback goes past the
    // second's optimistic update, and it is not the last to settle.
    await act(async () => refuseFirst(new Error("500")));
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(shown()).toEqual([close.key, payroll.key]);
    expect(service.list).not.toHaveBeenCalled();

    // The second lands. The server holds both hidden, so the UI has to read that back.
    await act(async () => landSecond(serverPrefs));
    await waitFor(() => expect(service.list).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(shown()).toEqual([]));
    expect(qc.getQueryData<HomeWidgetsResponse>(widgetsKey)?.hidden?.map((h) => h.key)).toEqual([
      close.key,
      payroll.key
    ]);
    await waitFor(() => expect(qc.getQueryData(prefsKey)).toEqual(serverPrefs));
  });

  it("reads the widgets again only once per failed burst", async () => {
    service.savePreferences.mockRejectedValueOnce(new Error("500"));
    const result = setup();
    act(() => result.current.hide(card("close")));
    await waitFor(() => expect(service.list).toHaveBeenCalledTimes(1));
    // A later, clean change does not inherit the earlier failure.
    act(() => result.current.setToday("hidden"));
    await waitFor(() => expect(service.savePreferences).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(qc.isMutating()).toBe(0));
    expect(service.list).toHaveBeenCalledTimes(1);
  });
});
