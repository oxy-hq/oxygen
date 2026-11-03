import { monogram } from "@/components/apps/AppMark";
import type { CustomAppSummary } from "@/types/apps";

/**
 * The note the home leaves for the app it is opening, in `sessionStorage`. The
 * custom-app runtime reads it as the app's document starts and draws the same
 * launch screen the home was showing, until the app has drawn itself
 * (`runtime.js`, "launch screen"). Same origin and same tab only, which is
 * exactly the case it is for: an app opened from the home, in the home's tab.
 *
 * Everything the runtime needs is in the note, measured from the home as drawn,
 * so the two screens match to the pixel and the app document loads nothing for it.
 */
export const LAUNCH_HANDOFF_KEY = "oxy.launch";

export type LaunchHandoff = {
  /** The app's root path: the runtime ignores a note for any other app. */
  root: string;
  name: string;
  /** The icon's URL, or `null` for the monogram. */
  icon: string | null;
  /** The monogram's letters, drawn when there is no icon. */
  mark: string;
  /** The monogram's ground. */
  tint: string;
  /** The home's background, text and border colours, and its typeface. */
  bg: string;
  fg: string;
  line: string;
  font: string;
  /** When the person pressed: a stale note (a navigation that never landed) is ignored. */
  at: number;
};

/** The app a link opens, and the app's root URL, or `null` for any other link. */
export function appFor<App extends Pick<CustomAppSummary, "url">>(
  href: string,
  apps: readonly App[]
): { app: App; root: URL } | null {
  let target: URL;
  try {
    target = new URL(href, window.location.href);
  } catch {
    return null;
  }
  let best: { app: App; root: URL } | null = null;
  for (const app of apps) {
    let root: URL;
    try {
      root = new URL(app.url, window.location.href);
    } catch {
      continue;
    }
    const prefix = root.href.replace(/\/*$/, "/");
    const opens = `${target.origin}${target.pathname}/`.startsWith(prefix);
    // The longest root wins, should one app's path ever sit under another's.
    if (opens && (!best || root.href.length > best.root.href.length)) best = { app, root };
  }
  return best;
}

/** Leave the note for `app`, drawn as `mark` (its icon on the pressed tile or card) is. */
export function leaveHandoff(app: CustomAppSummary, root: URL, mark: Element | null) {
  // Another origin cannot read this tab's storage: the app shows its own first frame.
  if (root.origin !== window.location.origin) return;
  const page = getComputedStyle(document.documentElement);
  const body = getComputedStyle(document.body);
  const icon =
    mark instanceof HTMLImageElement ? mark.currentSrc || mark.src : mark ? null : app.icon_url;
  const note: LaunchHandoff = {
    root: root.pathname,
    name: app.name,
    icon: icon || null,
    mark: monogram(app.name, 2),
    tint: mark && !icon ? getComputedStyle(mark).backgroundColor : "transparent",
    bg: body.backgroundColor,
    fg: body.color,
    line: page.getPropertyValue("--border").trim(),
    font: body.fontFamily,
    at: Date.now()
  };
  try {
    window.sessionStorage.setItem(LAUNCH_HANDOFF_KEY, JSON.stringify(note));
  } catch {
    // Storage refused: the app opens on its own first frame, as it always did.
  }
}

/** Take the note back: the open did not happen (Escape, or Back restored the home). */
export function clearHandoff() {
  try {
    window.sessionStorage.removeItem(LAUNCH_HANDOFF_KEY);
  } catch {
    // Nothing was written, or nothing can be.
  }
}
