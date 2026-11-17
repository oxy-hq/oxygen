import { create } from "zustand";
import { persist } from "zustand/middleware";

export type ThemeMode = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

interface ThemeState {
  mode: ThemeMode;
  theme: ResolvedTheme;
  setMode: (mode: ThemeMode) => void;
}

const SYSTEM_DARK_QUERY = "(prefers-color-scheme: dark)";

const getSystemTheme = (): ResolvedTheme => {
  if (typeof window === "undefined" || !window.matchMedia) {
    return "light";
  }
  return window.matchMedia(SYSTEM_DARK_QUERY).matches ? "dark" : "light";
};

const resolveTheme = (mode: ThemeMode, systemTheme: ResolvedTheme): ResolvedTheme =>
  mode === "system" ? systemTheme : mode;

export const isThemeMode = (value: unknown): value is ThemeMode =>
  value === "light" || value === "dark" || value === "system";

/** Where this browser keeps its own copy of the mode. It paints the first frame
 *  before the server's copy (`stores/themeSync.ts`) is read. */
export const THEME_STORAGE_KEY = "theme-storage";

/** Oxygen's mode for a person who has not chosen one. */
export const DEFAULT_THEME_MODE: ThemeMode = "light";

const useTheme = create<ThemeState>()(
  persist(
    (set) => ({
      mode: DEFAULT_THEME_MODE,
      theme: DEFAULT_THEME_MODE,
      setMode: (mode) => {
        const theme = resolveTheme(mode, getSystemTheme());
        set({ mode, theme });
      }
    }),
    {
      name: THEME_STORAGE_KEY,
      partialize: (state) => ({ mode: state.mode }),
      merge: (persisted, current) => {
        const p = persisted as { mode?: unknown } | null;
        const mode: ThemeMode = isThemeMode(p?.mode) ? p.mode : current.mode;
        return { ...current, mode, theme: resolveTheme(mode, getSystemTheme()) };
      }
    }
  )
);

export default useTheme;
