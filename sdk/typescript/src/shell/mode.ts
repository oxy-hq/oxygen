// One light/dark mode per person, shared by Oxygen and every custom app (decision 19: every app
// has both modes and follows Oxygen home; a change in an app changes Oxygen too).
//
// - The shell context carries the person's `mode` (`null` until they choose; absent for a crew
//   sign-in and on an older server). `useShellMode` applies it, keeps this origin's copy
//   (`localStorage["oxy-person-mode"]`, which the pre-paint applies before first paint), re-reads it
//   when the tab is shown again (`GET /api/projects/:id/shell-context/mode`), and writes a choice
//   from the shell's switch (`PUT` the same path), so Oxygen and the person's other apps follow.
// - On a staging or sandbox host (`window.__OXY_APP__.environment` `staging` or `dev-<handle>`) a
//   choice is that preview's only (Mars, 16 Nov): it is kept in this origin's copy, marked as made
//   here, and never sent, so a preview never changes the person's real mode; the server refuses
//   the write there too. Until a tester chooses, the preview shows the person's mode as read.
// - An app pins one mode with `<html data-oxy-mode-pin="light">`, or for one screen with
//   `useOxyModePin("light")` / `<OxyShell modePin="light">`: the pin wins and the switch hides.
//   While a pin is in force this origin's copy is left as it is, so a screen that is nobody's (a
//   store's shared tablet) never keeps the signed-in person's mode for the next load to paint.
// - None of it acts without theme.css (`isOxyThemeApplied`): an app that never imported the theme
//   keeps its own look, its own `.dark` styles included, exactly as on 2.28.1.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore
} from "react";
import { readAppEnvironment } from "../custom-app/inject";
import { getOxyAppLogger } from "../custom-app/logger";
import { useOxyApp } from "../custom-app/react";
import {
  applyOxyMode,
  isOxyModeChoice,
  isOxyThemeApplied,
  OXY_MODE_PIN_ATTRIBUTE,
  OXY_PERSON_MODE_KEY,
  type OxyMode,
  type OxyModeChoice,
  readOxyModePin,
  readOxyPersonMode,
  readOxyPreviewMode,
  rememberOxyPersonMode,
  resolveOxyMode,
  syncOxyThemeColor
} from "../theme/index";
import type { ShellContextData } from "./shellContext";

/** Oxygen's mode for a person who has not chosen one (its web app's default). */
export const DEFAULT_MODE: OxyModeChoice = "light";

// ── A staging or sandbox preview ────────────────────────────────────────────────────────────────
// A choice made there stays on that origin (its copy, `OXY_PERSON_MODE_KEY`) and is never written
// to the person. This key marks the copy as the preview's own, so the person's mode, read from the
// shell context or again when the tab is shown, does not replace it (which would snap the switch
// back the way a refused write does).

/** On a preview origin: `"1"` when the copy of the mode is a choice made here, never sent. */
export const OXY_PREVIEW_MODE_CHOSEN_KEY = "oxy-person-mode-preview";

/**
 * Whether this page is a staging or sandbox preview (`window.__OXY_APP__.environment` is
 * `staging` or `dev-<handle>`), where the switch never writes the person's real mode. Production,
 * and a page oxy did not serve (no environment), write as before; the server decides there.
 */
export function isPreviewEnvironment(environment: unknown = readAppEnvironment()): boolean {
  return (
    environment === "staging" || (typeof environment === "string" && environment.startsWith("dev-"))
  );
}

function previewChoiceMade(): boolean {
  try {
    return window.localStorage.getItem(OXY_PREVIEW_MODE_CHOSEN_KEY) === "1";
  } catch {
    return false;
  }
}

function markPreviewChoice(): void {
  try {
    window.localStorage.setItem(OXY_PREVIEW_MODE_CHOSEN_KEY, "1");
  } catch {
    // Blocked storage: the choice still shows in this tab.
  }
}

// ── The pin ──────────────────────────────────────────────────────────────────────────────────────
// The attribute on <html> is the one source: an app can write it in index.html (seen by the
// pre-paint) or set it per screen with `useOxyModePin`. Listeners hear the hook's changes.

const pinListeners = new Set<() => void>();

function subscribePin(listener: () => void): () => void {
  pinListeners.add(listener);
  return () => pinListeners.delete(listener);
}

function currentPin(): OxyMode | null {
  return readOxyModePin();
}

/** The mode the app pins right now, or null. Re-renders when `useOxyModePin` changes it. */
export function useModePin(): OxyMode | null {
  return useSyncExternalStore(subscribePin, currentPin, () => null);
}

/**
 * Pin this screen to one mode while it is mounted (`null`/`undefined` pins nothing): it shows
 * `mode` whatever the person picked, and the shell's switch hides. For a screen that is never one
 * person's, such as a store's shared tablet. When it unmounts, whatever pin was there before (an
 * `index.html` pin, or none) comes back. Acts only with theme.css applied, like the mode itself.
 */
export function useOxyModePin(mode: OxyMode | null | undefined): void {
  useLayoutEffect(() => {
    if (mode !== "light" && mode !== "dark") return;
    const root = document.documentElement;
    const before = root.getAttribute(OXY_MODE_PIN_ATTRIBUTE);
    root.setAttribute(OXY_MODE_PIN_ATTRIBUTE, mode);
    for (const l of pinListeners) l();
    return () => {
      if (before === null) root.removeAttribute(OXY_MODE_PIN_ATTRIBUTE);
      else root.setAttribute(OXY_MODE_PIN_ATTRIBUTE, before);
      for (const l of pinListeners) l();
    };
  }, [mode]);
}

// ── The person's mode ───────────────────────────────────────────────────────────────────────────

/** What `useShellMode` gives `OxyShell`. */
export interface ShellMode {
  /** Draw the switch: the theme is applied, the server carries the person's mode, no pin. */
  switchable: boolean;
  /** The person's choice, `light` until they make one. */
  choice: OxyModeChoice;
  /**
   * Apply `next` now and write it to the person's copy; reverts if the server refuses. On a
   * staging or sandbox host it is kept on this origin only and never sent.
   */
  choose: (next: OxyModeChoice) => void;
}

/** Whether this shell context names the person's mode at all (a new server, a member). */
export function carriesMode(data: ShellContextData | null): boolean {
  return !!data && "mode" in data && data.user?.kind !== "frontline";
}

/** The shell context's `mode` as a choice: `null` or anything unknown is Oxygen's default. */
export function choiceOf(mode: unknown): OxyModeChoice {
  return isOxyModeChoice(mode) ? mode : DEFAULT_MODE;
}

/** The mode the page shows: the pin, else a staging preview's own mode, else the person's. */
export function modeToShow(
  pin: OxyMode | null,
  preview: OxyMode | null,
  choice: OxyModeChoice | null,
  prefersDark: boolean
): OxyMode | null {
  return pin ?? preview ?? (choice ? resolveOxyMode(choice, prefersDark) : null);
}

const DARK_QUERY = "(prefers-color-scheme: dark)";

/** The device's own light/dark setting, followed live once `enabled` (theme.css applied). */
function usePrefersDark(enabled: boolean): boolean {
  const [dark, setDark] = useState(() => {
    try {
      return typeof window !== "undefined" && !!window.matchMedia?.(DARK_QUERY).matches;
    } catch {
      return false;
    }
  });
  useEffect(() => {
    if (!enabled) return;
    let query: MediaQueryList | undefined;
    try {
      query = window.matchMedia?.(DARK_QUERY);
    } catch {
      query = undefined;
    }
    if (!query?.addEventListener) return;
    // Read the list, not the event: it is the same answer, and a list is always there.
    const onChange = () => setDark(!!query?.matches);
    query.addEventListener("change", onChange);
    return () => query?.removeEventListener("change", onChange);
  }, [enabled]);
  return dark;
}

/** Read the server's copy (`GET …/shell-context/mode`); undefined when it cannot be read. */
async function readServerMode(
  fetcher: ReturnType<typeof useOxyApp>["fetcher"],
  projectId: string
): Promise<OxyModeChoice | undefined> {
  try {
    const res = await fetcher(`/api/projects/${projectId}/shell-context/mode`, { method: "GET" });
    if (!res.ok) return undefined;
    const body = (await res.json()) as { mode?: unknown };
    return choiceOf(body?.mode);
  } catch {
    return undefined;
  }
}

/**
 * The person's mode inside the wired shell: apply it (with theme.css only), keep this origin's
 * copy, follow a change made elsewhere (tab shown again, another tab of this origin, the device's
 * own setting under `system`), and write a choice from the switch.
 */
export function useShellMode(data: ShellContextData | null): ShellMode {
  const { projectId, fetcher } = useOxyApp();
  const pin = useModePin();
  const [themed, setThemed] = useState(false);
  const prefersDark = usePrefersDark(themed);
  const [choice, setChoice] = useState<OxyModeChoice | null>(() => readOxyPersonMode());
  const live = carriesMode(data);
  const serverMode = live ? choiceOf(data?.mode) : null;
  const preview = isPreviewEnvironment();
  const choiceRef = useRef(choice);
  choiceRef.current = choice;

  useLayoutEffect(() => setThemed(isOxyThemeApplied()), []);

  const adopt = useCallback((next: OxyModeChoice) => setChoice(next), []);

  // This origin's copy, which the pre-paint applies on the next load: kept only while no pin is in
  // force (read from <html> now, so a pin a screen set this commit counts). A pinned screen, such as
  // a store's shared tablet, is nobody's, so the person signed in there leaves no mode behind; their
  // own screen writes it once the pin lifts.
  useEffect(() => {
    if (choice && !pin && !readOxyModePin()) rememberOxyPersonMode(choice);
  }, [choice, pin]);

  // The shell context's answer, each time it arrives; on a preview, until a tester chooses there.
  useEffect(() => {
    if (serverMode && !(preview && previewChoiceMade())) adopt(serverMode);
  }, [serverMode, preview, adopt]);

  // What the page shows. A layout effect, so a change lands before the frame paints.
  useLayoutEffect(() => {
    if (!themed) return;
    const mode = modeToShow(pin, readOxyPreviewMode(), choice, prefersDark);
    // applyOxyMode moves <meta name="theme-color"> with it; with no mode, the light it shows.
    if (mode) applyOxyMode(mode);
    else syncOxyThemeColor();
  }, [themed, pin, choice, prefersDark]);

  // The browser's chrome follows whoever changes the mode on <html> (an app's own `.dark` too), as
  // the apps' own theme-color hooks did; the shell's own changes sync it as they apply.
  useEffect(() => {
    if (!themed || typeof MutationObserver === "undefined") return;
    const observer = new MutationObserver(() => {
      syncOxyThemeColor();
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "data-oxy-mode"]
    });
    return () => observer.disconnect();
  }, [themed]);

  // A change made in Oxygen or another app: read again whenever the tab is shown.
  useEffect(() => {
    if (!live || !projectId) return;
    const onVisible = () => {
      if (document.hidden || (preview && previewChoiceMade())) return;
      void readServerMode(fetcher, projectId).then((next) => {
        if (next && next !== choiceRef.current) adopt(next);
      });
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [live, preview, projectId, fetcher, adopt]);

  // Another tab of this origin wrote its copy.
  useEffect(() => {
    if (!live) return;
    const onStorage = (e: StorageEvent) => {
      if (e.key === OXY_PERSON_MODE_KEY && isOxyModeChoice(e.newValue)) setChoice(e.newValue);
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [live]);

  const choose = useCallback(
    (next: OxyModeChoice) => {
      if (!projectId || !isOxyModeChoice(next)) return;
      const before = choiceRef.current ?? DEFAULT_MODE;
      adopt(next);
      if (preview) {
        // A staging or sandbox preview: this origin only, never the person's real mode.
        markPreviewChoice();
        return;
      }
      void fetcher(`/api/projects/${projectId}/shell-context/mode`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ mode: next })
      })
        .then((res) => {
          if (!res.ok) throw new Error(`HTTP ${res.status}`);
        })
        .catch((e: unknown) => {
          // The server kept the old mode, so the page shows it again rather than disagree with
          // Oxygen until the next read.
          adopt(before);
          getOxyAppLogger().log("warn", "saving the light/dark mode failed", {
            error: e instanceof Error ? e.message : String(e)
          });
        });
    },
    [projectId, fetcher, adopt, preview]
  );

  return { switchable: themed && live && !pin, choice: choice ?? DEFAULT_MODE, choose };
}
