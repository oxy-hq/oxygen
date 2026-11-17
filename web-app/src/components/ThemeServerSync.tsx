import { useEffect } from "react";
import { pullThemeMode } from "@/stores/themeSync";

/**
 * Keeps the light/dark mode in step with the person's server copy, which every
 * custom app shares: read once signed in, and again whenever the tab is shown.
 * Renders nothing. Mounted under the cloud router's signed-in tree only.
 */
export function ThemeServerSync() {
  useEffect(() => {
    void pullThemeMode();
    const onVisible = () => {
      if (document.visibilityState === "visible") void pullThemeMode();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, []);
  return null;
}
