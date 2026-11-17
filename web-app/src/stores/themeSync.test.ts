// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

const get = vi.fn();
const save = vi.fn();
vi.mock("@/services/api/appearance", () => ({
  AppearanceService: { get: () => get(), save: (mode: string) => save(mode) }
}));

import {
  chooseThemeMode,
  decide,
  localChoice,
  pullThemeMode,
  resetThemeSyncForTests,
  THEME_UPLOADED_KEY
} from "./themeSync";
import useTheme, { THEME_STORAGE_KEY } from "./useTheme";

const stored = (mode: unknown) => JSON.stringify({ state: { mode }, version: 0 });

beforeEach(() => {
  localStorage.clear();
  get.mockReset();
  save.mockReset();
  save.mockResolvedValue({ mode: null });
  resetThemeSyncForTests();
  useTheme.setState({ mode: "light", theme: "light" });
});

describe("decide", () => {
  it("adopts the server's mode when it differs, and does nothing when it matches", () => {
    expect(decide("dark", "light", "light", false)).toEqual({ kind: "adopt", mode: "dark" });
    expect(decide("dark", null, "dark", true)).toEqual({ kind: "none" });
  });

  it("writes this browser's choice up when the server has none, once per browser", () => {
    expect(decide(null, "dark", "dark", false)).toEqual({ kind: "upload", mode: "dark" });
    // Another account on the same browser: the person has not chosen, so Oxygen's default.
    expect(decide(null, "dark", "dark", true)).toEqual({ kind: "adopt", mode: "light" });
  });

  it("keeps the default for a person who never chose, on a browser that never did", () => {
    expect(decide(null, null, "light", false)).toEqual({ kind: "none" });
  });
});

describe("localChoice", () => {
  it("reads the persisted store's mode and nothing else", () => {
    localStorage.setItem(THEME_STORAGE_KEY, stored("system"));
    expect(localChoice(localStorage)).toBe("system");
    for (const raw of [stored("sepia"), "not json", JSON.stringify({ mode: "dark" })]) {
      localStorage.setItem(THEME_STORAGE_KEY, raw);
      expect(localChoice(localStorage)).toBeNull();
    }
    localStorage.removeItem(THEME_STORAGE_KEY);
    expect(localChoice(localStorage)).toBeNull();
  });
});

describe("pullThemeMode", () => {
  it("applies the server's mode over this browser's copy", async () => {
    get.mockResolvedValue({ mode: "dark" });
    await pullThemeMode();
    expect(useTheme.getState().mode).toBe("dark");
    expect(save).not.toHaveBeenCalled();
  });

  it("writes this browser's mode up when the server has none, and marks it", async () => {
    localStorage.setItem(THEME_STORAGE_KEY, stored("dark"));
    useTheme.setState({ mode: "dark", theme: "dark" });
    get.mockResolvedValue({ mode: null });
    await pullThemeMode();
    expect(save).toHaveBeenCalledWith("dark");
    expect(localStorage.getItem(THEME_UPLOADED_KEY)).toBe("1");
    expect(useTheme.getState().mode).toBe("dark");
  });

  it("does not mark the browser when the write up fails, so the next load tries again", async () => {
    localStorage.setItem(THEME_STORAGE_KEY, stored("dark"));
    get.mockResolvedValue({ mode: null });
    save.mockRejectedValue(new Error("500"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await pullThemeMode();
    expect(localStorage.getItem(THEME_UPLOADED_KEY)).toBeNull();
    warn.mockRestore();
  });

  it("leaves this browser's copy alone when the server does not answer (crew, old server)", async () => {
    useTheme.setState({ mode: "dark", theme: "dark" });
    get.mockRejectedValue(Object.assign(new Error("404"), { response: { status: 404 } }));
    await pullThemeMode();
    expect(useTheme.getState().mode).toBe("dark");
    expect(save).not.toHaveBeenCalled();
  });

  it("writes up a choice made while the read was in flight instead of overwriting it", async () => {
    get.mockResolvedValue({ mode: "light" });
    await pullThemeMode();
    let answer: (v: { mode: string }) => void = () => {};
    get.mockReturnValue(new Promise((r) => (answer = r)));
    const pulling = pullThemeMode();
    chooseThemeMode("system");
    answer({ mode: "dark" });
    await pulling;
    expect(useTheme.getState().mode).toBe("system");
    expect(save).toHaveBeenCalledWith("system");
  });
});

describe("chooseThemeMode", () => {
  it("writes both copies once the server has answered", async () => {
    get.mockResolvedValue({ mode: "light" });
    await pullThemeMode();
    chooseThemeMode("dark");
    expect(useTheme.getState().mode).toBe("dark");
    expect(save).toHaveBeenCalledWith("dark");
  });

  it("changes only this browser's copy before then, and writes it up on the next read", async () => {
    chooseThemeMode("dark");
    expect(save).not.toHaveBeenCalled();
    get.mockResolvedValue({ mode: "light" });
    await pullThemeMode();
    expect(useTheme.getState().mode).toBe("dark");
    expect(save).toHaveBeenCalledWith("dark");
  });
});
