// One light/dark mode per person, shared by Oxygen and every custom app.
//
// This browser's copy (`theme-storage`, the `useTheme` store) paints the first
// frame, so nothing flashes; then the server's copy (`GET /api/me/appearance`)
// applies, and it is read again whenever the tab is shown, so a change made in
// an app (or another tab, or another device) shows up here. A change made here
// writes both. The first time the server has no mode for the person but this
// browser has one, the browser's is written up, once per browser, so nobody's
// earlier choice is lost.
//
// A crew sign-in, local mode and a server older than the route all answer 404
// (or nothing): the browser's copy is then the only one, as before.

import { AppearanceService } from "@/services/api/appearance";
import useTheme, {
  DEFAULT_THEME_MODE,
  isThemeMode,
  THEME_STORAGE_KEY,
  type ThemeMode
} from "./useTheme";

/** Set once this browser's copy has been written up to a server that had none. */
export const THEME_UPLOADED_KEY = "theme-storage-uploaded";

type ReadableStorage = Pick<Storage, "getItem">;

/** The mode this browser's own copy holds, or null when it holds none. */
export function localChoice(storage: ReadableStorage | null): ThemeMode | null {
  try {
    const raw = storage?.getItem(THEME_STORAGE_KEY);
    if (!raw) return null;
    const mode = (JSON.parse(raw) as { state?: { mode?: unknown } } | null)?.state?.mode;
    return isThemeMode(mode) ? mode : null;
  } catch {
    return null;
  }
}

/** What one read of the server's copy leads to. */
export type SyncStep =
  | { kind: "adopt"; mode: ThemeMode }
  | { kind: "upload"; mode: ThemeMode }
  | { kind: "none" };

/**
 * Pure: the server's mode wins when it has one. When it has none, this
 * browser's own choice goes up, unless this browser already wrote one up (then
 * this is another account on a shared browser); otherwise the person has not
 * chosen and Oxygen's default applies, as it does in every app.
 */
export function decide(
  server: ThemeMode | null,
  local: ThemeMode | null,
  current: ThemeMode,
  uploadedBefore: boolean
): SyncStep {
  if (server) return server === current ? { kind: "none" } : { kind: "adopt", mode: server };
  if (local && !uploadedBefore) return { kind: "upload", mode: local };
  return current === DEFAULT_THEME_MODE
    ? { kind: "none" }
    : { kind: "adopt", mode: DEFAULT_THEME_MODE };
}

let live = false;
let edits = 0;
let pendingLocal = false;

function storage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

function uploadedBefore(): boolean {
  try {
    return !!storage()?.getItem(THEME_UPLOADED_KEY);
  } catch {
    return false;
  }
}

function markUploaded() {
  try {
    storage()?.setItem(THEME_UPLOADED_KEY, "1");
  } catch {
    // Blocked storage: the next load may write it up again, which is harmless.
  }
}

/** Write the mode up; true when the server kept it. Never throws. */
function save(mode: ThemeMode): Promise<boolean> {
  return AppearanceService.save(mode).then(
    () => true,
    (error: unknown) => {
      console.warn("Saving the light/dark mode failed", error);
      return false;
    }
  );
}

/**
 * Read the server's copy and apply it. Safe to call on every tab show: a read
 * that fails leaves the browser's copy in charge, and a choice made while the
 * read was in flight is written up rather than overwritten.
 */
export async function pullThemeMode(): Promise<void> {
  const before = edits;
  let server: ThemeMode | null;
  try {
    server = (await AppearanceService.get()).mode;
  } catch {
    return;
  }
  live = true;
  const current = useTheme.getState().mode;
  if (pendingLocal || edits !== before) {
    pendingLocal = false;
    await save(current);
    return;
  }
  const step = decide(server, localChoice(storage()), current, uploadedBefore());
  if (step.kind === "adopt") useTheme.getState().setMode(step.mode);
  if (step.kind === "upload" && (await save(step.mode))) markUploaded();
}

/** The person picked a mode (the theme toggle, Settings → Appearance). */
export function chooseThemeMode(mode: ThemeMode): void {
  edits += 1;
  useTheme.getState().setMode(mode);
  if (live) void save(mode);
  else pendingLocal = true;
}

/** Test seam: forget what this module learned. */
export function resetThemeSyncForTests(): void {
  live = false;
  edits = 0;
  pendingLocal = false;
}
