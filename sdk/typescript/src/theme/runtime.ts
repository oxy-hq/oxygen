// The theme runtime, written once as ONE self-contained function so that the inline pre-paint
// script (`OXY_THEME_PREPAINT` in ./index.ts) can carry its source text verbatim. Nothing inside
// `oxyThemeRuntime` may reach outside it: no imports, no module-level names, only its parameter and
// the browser globals a page always has (`URLSearchParams`). runtime.test.ts runs the derived string
// in a real page and checks it against the exported functions, so a reference that escapes shows up
// as a disagreement there, not as a silently dead preview.

/** A colour mode. */
export type OxyMode = "light" | "dark";

/** What a person picks: a mode, or `system` to follow the device. */
export type OxyModeChoice = OxyMode | "system";

/** What `readOxyThemeSelection` persists to: `sessionStorage` by default. */
export interface OxyThemeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** The element the attributes go on: `document.documentElement` by default. */
export interface OxyThemeRoot {
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
  /** Read for the app's pin (`data-oxy-mode-pin`); a root without it has no pin. */
  getAttribute?(name: string): string | null;
  /** `contains` lets the pre-paint take back a mode it put on before theme.css was applied. */
  readonly classList: {
    toggle(token: string, force?: boolean): boolean;
    contains?(token: string): boolean;
  };
}

/** The computed style the pre-paint reads `--oxy-theme-contract` from. */
export interface OxyThemeComputedStyle {
  getPropertyValue(name: string): string;
}

/** A `<meta name="theme-color">` as the runtime writes it. */
export interface OxyThemeMeta {
  setAttribute(name: string, value: string): void;
  /** On a `<link rel="stylesheet">`: its sheet, there once it has loaded. */
  readonly sheet?: unknown;
}

/** The parts of `document` the runtime reads and writes. */
export interface OxyThemeDocument {
  documentElement?: OxyThemeRoot | null;
  body?: OxyThemeRoot | null;
  createElement?(tag: "style"): { textContent: string | null };
  /** `complete` once the page has loaded, its stylesheets included (loaded or failed). */
  readonly readyState?: string;
  head?: {
    insertAdjacentHTML?(position: "beforeend", html: string): void;
    appendChild?(node: never): unknown;
    removeChild?(node: never): unknown;
  } | null;
  querySelectorAll?(selector: string): ArrayLike<OxyThemeMeta>;
}

/** The parts of `window` the pre-paint reads. */
export interface OxyThemeWindow {
  __OXY_APP__?: { environment?: unknown };
  location?: { hostname?: string; search?: string };
  sessionStorage?: OxyThemeStorage;
  /** Where the shell keeps this origin's copy of the person's mode. */
  localStorage?: OxyThemeStorage;
  matchMedia?: (query: string) => { matches: boolean };
  getComputedStyle?(element: OxyThemeRoot): OxyThemeComputedStyle;
  document?: OxyThemeDocument;
  CSS?: { supports?(property: string, value: string): boolean };
  /** The page's own observer; `observe` takes the real `<html>` (`never` here, as it is no root type). */
  MutationObserver?: new (
    callback: () => void
  ) => {
    observe(target: never, options: { attributes: boolean; attributeFilter: string[] }): void;
  };
  /** Runs its callback before the next frame is painted; the pre-paint settles there. */
  requestAnimationFrame?: (callback: () => void) => unknown;
}

/** What the page asked for, after the allow-list. */
export interface OxyThemeRuntimeSelection {
  theme: string;
  mode: OxyMode;
}

/** A theme and mode to apply; null or absent removes the attribute. */
export interface OxyThemeRuntimeChoice {
  theme?: string | null;
  mode?: OxyMode | null;
}

export interface OxyThemeWindowInput {
  environment: unknown;
  hostname: unknown;
  search: string;
  storage: OxyThemeStorage | null;
  /** `localStorage`, where this origin's copy of the person's mode is kept. */
  local: OxyThemeStorage | null;
  prefersDark: boolean;
}

export function oxyThemeRuntime(ids: readonly string[]) {
  const THEME_KEY = "oxy-theme";
  const MODE_KEY = "oxy-mode";
  /** This origin's copy of the person's mode (`localStorage`), written by the shell. */
  const PERSON_KEY = "oxy-person-mode";
  /** On `<html>`: the app shows only this mode, whatever the person picked. */
  const PIN_ATTRIBUTE = "data-oxy-mode-pin";

  function isTheme(value: unknown): value is string {
    return typeof value === "string" && ids.indexOf(value) !== -1;
  }

  function isMode(value: unknown): value is OxyMode {
    return value === "light" || value === "dark";
  }

  function isChoice(value: unknown): value is OxyModeChoice {
    return isMode(value) || value === "system";
  }

  /** The mode a choice shows: `system` follows the device. */
  function resolve(choice: OxyModeChoice, prefersDark: boolean): OxyMode {
    return choice === "system" ? (prefersDark ? "dark" : "light") : choice;
  }

  /**
   * Whether theme.css is applied: it is the only file that sets `--oxy-theme-contract`. Without it
   * the person's mode, the pin and the shell's switch do nothing, so an app that never imported the
   * theme looks exactly as it did (an app's own `.dark` styles included).
   */
  function themed(w: OxyThemeWindow, root: OxyThemeRoot): boolean {
    try {
      if (typeof w.getComputedStyle !== "function") return false;
      const v = w.getComputedStyle(root).getPropertyValue("--oxy-theme-contract").trim();
      return v !== "" && v !== "0";
    } catch {
      return false;
    }
  }

  /** The mode the app pinned on `<html data-oxy-mode-pin>`, or null. */
  function pinned(root: OxyThemeRoot): OxyMode | null {
    try {
      const v = typeof root.getAttribute === "function" ? root.getAttribute(PIN_ATTRIBUTE) : null;
      return isMode(v) ? v : null;
    } catch {
      return null;
    }
  }

  /** The person's mode as this origin last heard it, or null. */
  function person(local: OxyThemeStorage | null): OxyModeChoice | null {
    const v = stored(local, PERSON_KEY);
    return isChoice(v) ? v : null;
  }

  /**
   * The mode the page shows, with the theme applied: the app's pin, else a staging preview's own
   * mode (`?oxy-mode`, kept for the session; only where the preview is allowed), else the person's
   * mode. Null when none of them says anything, which leaves the page as it is.
   */
  function chosen(input: OxyThemeWindowInput, root: OxyThemeRoot): OxyMode | null {
    const pin = pinned(root);
    if (pin) return pin;
    if (allowed(input.environment, input.hostname)) {
      const preview = stored(input.storage, MODE_KEY);
      if (isMode(preview)) return preview;
    }
    const choice = person(input.local);
    return choice ? resolve(choice, input.prefersDark) : null;
  }

  /** Set data-oxy-mode and mirror it as .dark, leaving data-oxy-theme alone. False = refused. */
  function applyMode(mode: unknown, root: OxyThemeRoot): boolean {
    if (!isMode(mode)) return false;
    root.setAttribute("data-oxy-mode", mode);
    root.classList.toggle("dark", mode === "dark");
    return true;
  }

  /**
   * With theme.css applied, the browser's own chrome (`<meta name="theme-color">`: a phone
   * browser's toolbar, an installed app's status bar) in the page ground of the mode in force:
   * `--background` on `root` as it computes now (the theme's #f6f6f7 light, #0a0a0a dark, or the
   * app's own `--background` where it declares one), so a pin and `system` land as the page does.
   * Every theme-color meta takes it; a page with none gets one at the end of <head>. It reads the
   * token, not <body>, whose colour may still be easing out of the old mode. Without the theme it
   * does nothing and returns null, so an app that never imported it keeps its own meta.
   */
  function themeColor(w: OxyThemeWindow, root: OxyThemeRoot): string | null {
    try {
      if (!themed(w, root) || typeof w.getComputedStyle !== "function") return null;
      const ground = w.getComputedStyle(root).getPropertyValue("--background").trim();
      if (!ground) return null;
      const css = w.CSS;
      if (css && typeof css.supports === "function" && !css.supports("color", ground)) return null;
      const doc = w.document;
      if (!doc || typeof doc.querySelectorAll !== "function") return null;
      const selector = 'meta[name="theme-color"]';
      let metas = doc.querySelectorAll(selector);
      if (metas.length === 0 && doc.head && typeof doc.head.insertAdjacentHTML === "function") {
        doc.head.insertAdjacentHTML("beforeend", '<meta name="theme-color">');
        metas = doc.querySelectorAll(selector);
      }
      for (let i = 0; i < metas.length; i++) metas[i].setAttribute("content", ground);
      return metas.length > 0 ? ground : null;
    } catch {
      return null;
    }
  }

  /**
   * Keep theme-color on the ground for the page's life, whoever changes the mode on `root` (the
   * shell, an app's own `.dark`, a pin): `themeColor` again whenever its class or data-oxy-mode
   * changes. Only with theme.css applied; it watches `root`'s own attributes, not the meta it writes.
   */
  let following = false;
  function followThemeColor(w: OxyThemeWindow, root: OxyThemeRoot): void {
    try {
      if (following || !themed(w, root) || typeof w.MutationObserver !== "function") return;
      following = true;
      new w.MutationObserver(() => {
        themeColor(w, root);
      }).observe(root as never, { attributes: true, attributeFilter: ["class", "data-oxy-mode"] });
    } catch {
      // No observer: the meta still follows every mode the shell and the pre-paint apply.
    }
  }

  /**
   * A preview may re-theme staging and dev-<handle> sandboxes, and a page served from this
   * machine (a local `oxy start` reports "production"). Production on a real host, an unknown
   * environment on a real host, and anything else never.
   */
  function allowed(environment: unknown, hostname: unknown): boolean {
    if (environment === "staging") return true;
    if (typeof environment === "string" && environment.indexOf("dev-") === 0) return true;
    if (typeof hostname !== "string") return false;
    const host = hostname.toLowerCase();
    if (host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "[::1]")
      return true;
    return host.length > ".localhost".length && host.slice(-".localhost".length) === ".localhost";
  }

  function stored(storage: OxyThemeStorage | null, key: string): string | null {
    if (!storage) return null;
    try {
      return storage.getItem(key);
    } catch {
      return null;
    }
  }

  /**
   * ?oxy-theme and ?oxy-mode, allow-listed, persisted for the session; else what the session
   * persisted; the mode falls back to the system preference. A mode on its own previews the
   * shipped direction (the first id): importing the CSS already applies it, so the mode is the
   * preview that matters. A query naming a theme off the allow-list is refused whole, so nothing
   * from it applies or persists. Null when neither a theme nor a mode is chosen.
   */
  function read(
    search: string,
    storage: OxyThemeStorage | null,
    prefersDark: boolean
  ): OxyThemeRuntimeSelection | null {
    let askedTheme: string | null = null;
    let askedMode: string | null = null;
    try {
      const params = new URLSearchParams(search);
      askedTheme = params.get(THEME_KEY);
      askedMode = params.get(MODE_KEY);
    } catch {
      askedTheme = null;
    }
    const queryTheme = isTheme(askedTheme) ? askedTheme : null;
    if (askedTheme !== null && !queryTheme) return null;
    const queryMode = isMode(askedMode) ? askedMode : null;
    if (storage) {
      try {
        if (queryTheme) storage.setItem(THEME_KEY, queryTheme);
        if (queryMode) storage.setItem(MODE_KEY, queryMode);
      } catch {
        // Storage full or blocked: the choice still applies to this page.
      }
    }
    const keptTheme = stored(storage, THEME_KEY);
    const keptMode = stored(storage, MODE_KEY);
    const chosenMode = queryMode || (isMode(keptMode) ? keptMode : null);
    const theme =
      queryTheme ||
      (isTheme(keptTheme) ? keptTheme : null) ||
      (chosenMode && ids.length > 0 ? ids[0] : null);
    if (!theme) return null;
    const mode = chosenMode || (prefersDark ? "dark" : "light");
    return { theme, mode };
  }

  /** Set or remove data-oxy-theme and data-oxy-mode; mirror the mode as .dark. False = refused. */
  function apply(choice: OxyThemeRuntimeChoice, root: OxyThemeRoot): boolean {
    if (!choice || typeof choice !== "object") return false;
    const theme = choice.theme;
    const mode = choice.mode;
    if (theme != null && !isTheme(theme)) return false;
    if (mode != null && !isMode(mode)) return false;
    if (theme == null) root.removeAttribute("data-oxy-theme");
    else root.setAttribute("data-oxy-theme", theme);
    if (mode == null) {
      root.removeAttribute("data-oxy-mode");
    } else {
      root.setAttribute("data-oxy-mode", mode);
      root.classList.toggle("dark", mode === "dark");
    }
    return true;
  }

  /** What the pre-paint reads from a window, each read guarded (storage can throw). */
  function fromWindow(w: OxyThemeWindow): OxyThemeWindowInput {
    const location = w.location || {};
    let storage: OxyThemeStorage | null = null;
    try {
      storage = w.sessionStorage || null;
    } catch {
      storage = null;
    }
    let local: OxyThemeStorage | null = null;
    try {
      local = w.localStorage || null;
    } catch {
      local = null;
    }
    let prefersDark = false;
    try {
      prefersDark =
        typeof w.matchMedia === "function" && w.matchMedia("(prefers-color-scheme: dark)").matches;
    } catch {
      prefersDark = false;
    }
    const app = w.__OXY_APP__;
    return {
      environment: app ? app.environment : undefined,
      hostname: location.hostname,
      search: location.search || "",
      storage,
      local,
      prefersDark
    };
  }

  /** `root`'s mode as it stands (data-oxy-mode and `.dark`), to tell whether anyone changed it. */
  function current(root: OxyThemeRoot): string {
    try {
      const mode =
        typeof root.getAttribute === "function" ? root.getAttribute("data-oxy-mode") : "?";
      const dark =
        typeof root.classList.contains === "function" ? root.classList.contains("dark") : "?";
      return `${mode}|${dark}`;
    } catch {
      return "?";
    }
  }

  /**
   * Whether a stylesheet the page links is still loading (no `sheet` yet), until the page has
   * loaded: before then the pre-paint does not decide that theme.css is missing.
   */
  function loading(w: OxyThemeWindow): boolean {
    try {
      const doc = w.document;
      if (!doc || doc.readyState === "complete" || typeof doc.querySelectorAll !== "function")
        return false;
      const links = doc.querySelectorAll('link[rel~="stylesheet"]');
      for (let i = 0; i < links.length; i++) if (!links[i].sheet) return true;
      return false;
    } catch {
      return false;
    }
  }

  /** `then` before the next frame is painted, or at once where there are no frames. */
  function beforeFrame(w: OxyThemeWindow, then: () => void): void {
    if (typeof w.requestAnimationFrame === "function") w.requestAnimationFrame(then);
    else then();
  }

  /**
   * Put the mode the page shows on `root`: the preview (gate, read, apply), then the pin, the
   * preview's mode or the person's (`chosen`). With theme.css applied that is final. Before it is
   * (the script runs in <head>, ahead of the stylesheets), the mode still goes on at once, so a
   * dark person's first frame is dark, and this returns how to take it back.
   */
  function show(w: OxyThemeWindow, root: OxyThemeRoot): (() => void) | null {
    // Read before anything changes, so no style is computed under the new mode here (see `still`).
    const styled = themed(w, root);
    const input = fromWindow(w);
    if (allowed(input.environment, input.hostname)) {
      const selection = read(input.search, input.storage, input.prefersDark);
      if (selection) apply(selection, root);
    }
    const mode = chosen(input, root);
    if (!mode) return null;
    if (styled) {
      applyMode(mode, root);
      return null;
    }
    if (typeof root.getAttribute !== "function" || typeof root.classList.contains !== "function")
      return null;
    const before = root.getAttribute("data-oxy-mode");
    const dark = root.classList.contains("dark");
    applyMode(mode, root);
    return () => {
      if (before === null) root.removeAttribute("data-oxy-mode");
      else root.setAttribute("data-oxy-mode", before);
      root.classList.toggle("dark", dark);
    };
  }

  /**
   * Let a mode the first frame's settling changes land without a transition. No frame has shown the
   * mode it replaces, but a style already computed under it makes a background ease from it: on
   * Store Ops' tablet, with the pre-paint ahead of its own pin script, the first frame had <html>
   * light and <body> still dark, one dark frame on screen. With every transition off, the new style
   * is computed once, then they come back.
   */
  function still(w: OxyThemeWindow): void {
    try {
      const doc = w.document;
      const head = doc ? doc.head : null;
      if (!doc || !head || typeof doc.createElement !== "function") return;
      if (typeof head.appendChild !== "function" || typeof head.removeChild !== "function") return;
      const style = doc.createElement("style");
      style.textContent = "*,*::before,*::after{transition:none!important}";
      head.appendChild(style as never);
      if (typeof w.getComputedStyle === "function" && doc.documentElement)
        w.getComputedStyle(doc.body || doc.documentElement).getPropertyValue("transition-duration");
      head.removeChild(style as never);
    } catch {
      // No transition to stop is better than a page the pre-paint broke.
    }
  }

  /**
   * The first frame after the pre-paint ran. A browser paints nothing until the stylesheets in
   * <head> have loaded and <body> has begun, and runs this just before that first paint. With
   * theme.css applied: if nobody has changed the mode since, choose it again, now that the
   * platform's `__OXY_APP__` (spliced in at the end of <head>) and a pin written after the script
   * are there; then theme-color, kept on the ground for the page's life. Without it: while a
   * linked stylesheet is still loading, wait a frame; then take back what the pre-paint put on,
   * unless someone has changed it since, so an app that never imported the theme looks as it did.
   * Either change lands without a transition (`still`).
   */
  function settle(
    w: OxyThemeWindow,
    root: OxyThemeRoot,
    left: string,
    undo: (() => void) | null
  ): void {
    try {
      if (themed(w, root)) {
        const was = current(root);
        if (was === left) show(w, root);
        if (current(root) !== was) still(w);
        themeColor(w, root);
        followThemeColor(w, root);
      } else if (loading(w)) {
        beforeFrame(w, () => settle(w, root, left, undo));
      } else if (undo && current(root) === left) {
        undo();
        still(w);
      }
    } catch {
      // A preview must never break the page it previews.
    }
  }

  /**
   * The pre-paint, placed anywhere in <head> (or at the top of <body>): `show`, then, with
   * theme.css applied, the browser's theme-color in the ground of that mode (`themeColor`), kept
   * there as the mode changes later (`followThemeColor`); and `settle` before the first frame,
   * for what a script in <head> cannot know yet. It never throws into the page.
   */
  function prepaint(w: OxyThemeWindow): void {
    try {
      const root = w.document ? w.document.documentElement : null;
      if (!root) return;
      const undo = show(w, root);
      const left = current(root);
      if (themed(w, root)) {
        themeColor(w, root);
        followThemeColor(w, root);
      }
      beforeFrame(w, () => settle(w, root, left, undo));
    } catch {
      // A preview must never break the page it previews.
    }
  }

  return {
    PERSON_KEY,
    PIN_ATTRIBUTE,
    isTheme,
    isMode,
    isChoice,
    resolve,
    allowed,
    read,
    apply,
    applyMode,
    themeColor,
    followThemeColor,
    themed,
    pinned,
    person,
    chosen,
    fromWindow,
    prepaint
  };
}
