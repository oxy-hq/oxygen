// `@oxy-hq/sdk/theme`: the theme's runtime half. Pure TS, no React. Optional: the CSS needs none of it.
//
// `@oxy-hq/sdk/theme.css` is the oxygen reference app's design (oxy-hq/reference-app) as the
// contract's custom properties, and importing it applies it: the light palette on `<html>`, the dark
// one under the `.dark` class or `data-oxy-mode="dark"`. Its selectors sit inside `:where()`, so an
// app's own token declarations still win; adopting the theme means importing it and deleting them.
// The SDK shell and phone kit read its names (`--radius-*`, `--rail-border`, `--topbar-border`,
// `--card-border`, `--control-h-lg`, …) before their old fallbacks, so an app that does not import it
// renders exactly as before.
//
// One light/dark mode per person, shared with Oxygen: with theme.css applied, `OxyShell` applies the
// signed-in person's mode (the shell context's `mode`, re-read when the tab is shown again) and puts
// a Light / Dark / System switch in the rail that writes it back, so Oxygen follows. This origin
// keeps a copy (`localStorage["oxy-person-mode"]`) that the pre-paint below applies before first
// paint. An app pins one mode for itself (and hides the switch) with `data-oxy-mode-pin="light"` on
// `<html>`, or for one screen with `useOxyModePin` / `<OxyShell modePin>` from `@oxy-hq/sdk/shell`.
// Without theme.css none of this acts: no mode is applied and no switch is drawn.
//
// This entry switches the mode (`applyOxyTheme({ mode: "dark" })` sets data-oxy-mode and mirrors it
// as the `.dark` class the shell keys on) and keeps the preview gate: `?oxy-mode=light|dark` (and
// `?oxy-theme=<id>`, for a direction beyond the shipped one) re-themes staging, dev-<handle> sandboxes
// and pages served from this machine, never production. One direction ships today, so
// `data-oxy-theme` selects nothing that importing the CSS has not already applied.
//
// A custom app that wants the person's mode (and the preview) without a flash puts
// OXY_THEME_PREPAINT in an inline <script> in <head>, anywhere there, before or after its
// stylesheets. Not at the top of <body>: by then the browser has the stylesheet and a <body>, and
// Chrome may paint that empty, light page before the script runs. In <head> nothing paints until
// the stylesheets have loaded. Ahead of them the script cannot read theme.css's marker yet, so it
// puts the mode on <html> at once and settles the rest just before the first frame: a pin written
// after it, the platform's `window.__OXY_APP__` (spliced in just before </head>, after the script)
// and theme-color; if theme.css never applied, it takes the mode back off.
//
//   // vite.config.ts
//   import { OXY_THEME_PREPAINT } from "@oxy-hq/sdk/theme";
//   transformIndexHtml: () => [{ tag: "script", children: OXY_THEME_PREPAINT, injectTo: "head" }]
import {
  type OxyMode,
  type OxyModeChoice,
  type OxyThemeRoot,
  type OxyThemeStorage,
  oxyThemeRuntime
} from "./runtime";
import { THEMES } from "./themes.gen";

export type {
  OxyMode,
  OxyModeChoice,
  OxyThemeComputedStyle,
  OxyThemeDocument,
  OxyThemeMeta,
  OxyThemeRoot,
  OxyThemeStorage,
  OxyThemeWindow
} from "./runtime";
export { THEMES };

/** A theme theme.css carries: its id (the data-oxy-theme value), name, carrier and summary. */
export type OxyTheme = (typeof THEMES)[number];
/** A theme id theme.css carries. */
export type OxyThemeId = OxyTheme["id"];

/** A theme and a mode to apply. `null` (or leaving it out) removes that attribute. */
export interface OxyThemeChoice {
  theme?: OxyThemeId | null;
  mode?: OxyMode | null;
}

/** What a page asked for: a direction and the mode to show it in. */
export interface OxyThemeSelection {
  theme: OxyThemeId;
  mode: OxyMode;
}

/** Where `readOxyThemeSelection` reads from. Each field defaults to the current window's. */
export interface OxyThemeSelectionSource {
  /** A query string such as `?oxy-theme=folio&oxy-mode=dark`. Default: `location.search`. */
  search?: string;
  /** Where the choice persists for the session; `null` for nowhere. Default: `sessionStorage`. */
  storage?: OxyThemeStorage | null;
  /** Whether the system prefers dark. Default: `matchMedia("(prefers-color-scheme: dark)")`. */
  prefersDark?: boolean;
}

const THEME_IDS: readonly OxyThemeId[] = THEMES.map((t) => t.id);
const runtime = oxyThemeRuntime(THEME_IDS);

/**
 * The attribute an app pins its mode with: `<html data-oxy-mode-pin="light">` shows light whatever
 * the person picked, and hides the shell's switch. `"light"` or `"dark"`; anything else is no pin.
 */
export const OXY_MODE_PIN_ATTRIBUTE: string = runtime.PIN_ATTRIBUTE;

/** Where this origin keeps its copy of the person's mode (`localStorage`). */
export const OXY_PERSON_MODE_KEY: string = runtime.PERSON_KEY;

/** True for `light`, `dark` and `system`. */
export function isOxyModeChoice(value: unknown): value is OxyModeChoice {
  return runtime.isChoice(value);
}

/** The mode a choice shows: `system` is the device's (`prefersDark`). */
export function resolveOxyMode(choice: OxyModeChoice, prefersDark: boolean): OxyMode {
  return runtime.resolve(choice, prefersDark);
}

/**
 * Whether theme.css is applied to `root` (it alone sets `--oxy-theme-contract`). The person's mode,
 * the pin and the shell's switch act only when it is.
 */
export function isOxyThemeApplied(root?: OxyThemeRoot): boolean {
  if (typeof window === "undefined") return false;
  const el = root ?? window.document?.documentElement;
  return !!el && runtime.themed(window, el);
}

/** The mode `root` pins (`data-oxy-mode-pin`), or null. */
export function readOxyModePin(root?: OxyThemeRoot): OxyMode | null {
  const el = root ?? (typeof document === "undefined" ? null : document.documentElement);
  return el ? runtime.pinned(el) : null;
}

/**
 * Set data-oxy-mode (and `.dark`) only, leaving data-oxy-theme alone. False for an unknown mode.
 * On the page's own `<html>` with theme.css applied, `<meta name="theme-color">` follows
 * (`syncOxyThemeColor`).
 */
export function applyOxyMode(
  mode: OxyMode,
  root: OxyThemeRoot = document.documentElement
): boolean {
  const applied = runtime.applyMode(mode, root);
  if (applied) syncThemeColorFor(root);
  return applied;
}

/**
 * With theme.css applied, set `<meta name="theme-color">` (a phone browser's toolbar, an installed
 * app's status bar) to the page ground of the mode in force, `--background` as `<html>` computes it
 * now (light #f6f6f7, dark #0a0a0a), adding the meta to `<head>` if the page has none. The shell
 * and the pre-paint call it whenever they apply a mode, pins included; call it after changing
 * `.dark` yourself. Returns the colour written, or null (no theme, no document): without the
 * theme it changes nothing.
 */
export function syncOxyThemeColor(): string | null {
  if (typeof window === "undefined" || !window.document) return null;
  return runtime.themeColor(window, window.document.documentElement);
}

function syncThemeColorFor(root: OxyThemeRoot): void {
  if (typeof window !== "undefined" && root === window.document?.documentElement)
    runtime.themeColor(window, root);
}

/** This origin's copy of the person's mode, or null. Never throws. */
export function readOxyPersonMode(): OxyModeChoice | null {
  if (typeof window === "undefined") return null;
  try {
    return runtime.person(window.localStorage);
  } catch {
    return null;
  }
}

/** Keep `choice` as this origin's copy of the person's mode. Never throws. */
export function rememberOxyPersonMode(choice: OxyModeChoice): void {
  if (typeof window === "undefined" || !runtime.isChoice(choice)) return;
  try {
    window.localStorage.setItem(runtime.PERSON_KEY, choice);
  } catch {
    // Blocked storage: this tab still shows the choice; the next load reads the server's.
  }
}

/**
 * The mode a staging preview chose explicitly (`?oxy-mode`, kept for the session), where the
 * preview gate allows one; null otherwise. It outranks the person's mode, as in the pre-paint.
 */
export function readOxyPreviewMode(): OxyMode | null {
  if (typeof window === "undefined") return null;
  const page = runtime.fromWindow(window);
  if (!runtime.allowed(page.environment, page.hostname)) return null;
  try {
    const v = page.storage ? page.storage.getItem("oxy-mode") : null;
    return runtime.isMode(v) ? v : null;
  } catch {
    return null;
  }
}

/** True for a direction id theme.css carries; the allow-list every other function applies. */
export function isOxyThemeId(value: unknown): value is OxyThemeId {
  return runtime.isTheme(value);
}

/**
 * Whether a `?oxy-theme` preview may re-theme this page. True for `environment` "staging" or
 * "dev-<handle>" (`window.__OXY_APP__.environment`), and for a page on this machine (localhost,
 * *.localhost, 127.0.0.1, ::1), which a local `oxy start` serves as "production". False for
 * "production" on any other host, and for a page oxy did not serve (no environment) off this machine.
 */
export function isOxyThemePreviewAllowed(
  environment: string | undefined,
  hostname: string | undefined
): boolean {
  return runtime.allowed(environment, hostname);
}

/**
 * Read `?oxy-theme` and `?oxy-mode` (allow-listed: unknown values are ignored, never written),
 * persist them for the session, else fall back to what the session persisted; the mode then falls
 * back to the system preference. A mode on its own previews the shipped direction; a query naming a
 * direction off the allow-list is refused whole. Null when neither a direction nor a mode is
 * chosen. It does not apply the gate:
 * call `isOxyThemePreviewAllowed` first, as the pre-paint does.
 */
export function readOxyThemeSelection(
  source: OxyThemeSelectionSource = {}
): OxyThemeSelection | null {
  const page = typeof window === "undefined" ? null : runtime.fromWindow(window);
  const search = source.search ?? page?.search ?? "";
  const storage = source.storage !== undefined ? source.storage : (page?.storage ?? null);
  const prefersDark = source.prefersDark ?? page?.prefersDark ?? false;
  const selection = runtime.read(search, storage, prefersDark);
  return selection && isOxyThemeId(selection.theme)
    ? { theme: selection.theme, mode: selection.mode }
    : null;
}

/**
 * Set (or, for null, remove) data-oxy-theme and data-oxy-mode on `root`, and mirror the mode as
 * the `.dark` class. An unknown id or mode is refused: nothing is written and it returns false.
 */
export function applyOxyTheme(
  choice: OxyThemeChoice,
  root: OxyThemeRoot = document.documentElement
): boolean {
  const applied = runtime.apply(choice, root);
  if (applied && choice.mode != null) syncThemeColorFor(root);
  return applied;
}

/**
 * The pre-paint as an inline-script body, for anywhere in `<head>`: the gate
 * (`isOxyThemePreviewAllowed`), the selection (`readOxyThemeSelection`) and `applyOxyTheme`, from
 * the same function those three call; the person's mode and the pin, on `<html>` at once; and, just
 * before the first frame, `<meta name="theme-color">` with theme.css applied, or the mode taken
 * back without it. It reads `window.__OXY_APP__`, `location`, `sessionStorage`, `localStorage` and
 * `matchMedia`, and never throws.
 */
export const OXY_THEME_PREPAINT = `(${oxyThemeRuntime.toString()})(${JSON.stringify(THEME_IDS)}).prepaint(window);`;
