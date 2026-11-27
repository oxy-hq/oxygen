import { apiErrorCode, apiErrorMessage, apiStatus } from "@/libs/apiError";
import type {
  BuildCheckChangedScreen,
  BuildCheckDetail,
  BuildCheckFailedRequest,
  BuildCheckNewError,
  BuildCheckSeen
} from "@/types/appBuildChecks";
import { readyText } from "./buildChecksModel";

/**
 * The pure half of the screen checks: `smoke` and `production_smoke` (each screen at
 * each viewport), and `changes` (each screen's two pictures and how much differs).
 *
 * A detail is JSON a runner wrote and a server stored as it came, and `screens` holds
 * a different shape per kind, so nothing here trusts a field to be there: a detail
 * with none of them (an older server, a row still queued) reads as no screens, never
 * as a throw.
 */

type Json = Record<string, unknown>;
const record = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const text = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const strings = (v: unknown): string[] => list(v).filter((x): x is string => typeof x === "string");
const count = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;

/** `/` for the root (`""`), `/stores` for `stores`. */
export function screenName(path: string): string {
  return `/${path.replace(/^\/+/, "")}`;
}

/** One viewport of one screen, as its line says it. */
export interface ViewportReading {
  name: string;
  /** The app said it was ready at this viewport. */
  ready: boolean;
  /** Ready, and nothing thrown on the page. */
  ok: boolean;
  /** `ready in 0.9s` / `never ready`. */
  text: string;
}

/** Something a screen showed, and at which viewports. */
export interface ScreenNote<T> {
  viewports: string[];
  value: T;
}

export interface ScreenReading {
  path: string;
  name: string;
  viewports: ViewportReading[];
  /** A viewport that never got ready, or threw. A failed request alone is not one. */
  failing: boolean;
  errors: ScreenNote<string>[];
  failedRequests: ScreenNote<BuildCheckFailedRequest>[];
  /** Writes the checker was refused, over every viewport: a warning only. */
  refusedWrites: number;
  /** Those of them the runner named (`POST /fn/save`), once each. */
  refused: string[];
  /** Requests to another origin the checker did not send, once each over every
   *  viewport: a checked page reaches only the app's own origin. A warning only. */
  offOrigin: string[];
}

/** Adds `value` under `viewport`, merging with the same value seen at another one. */
function note<T>(into: ScreenNote<T>[], viewport: string, value: T, same: (a: T, b: T) => boolean) {
  const seen = into.find((n) => same(n.value, value));
  if (!seen) into.push({ viewports: [viewport], value });
  else if (!seen.viewports.includes(viewport)) seen.viewports.push(viewport);
}

/** A request that failed, as the runner wrote it; null when its row names no URL. */
function failedRequest(raw: unknown): BuildCheckFailedRequest | null {
  if (!record(raw) || typeof raw.url !== "string") return null;
  return { url: raw.url, status: count(raw.status), error: text(raw.error) };
}

/** Adds one viewport of a screen to `reading`: its line, then what it showed there. */
function addViewport(reading: ScreenReading, v: Json) {
  const name = text(v.name) ?? "viewport";
  const pageErrors = strings(v.page_errors);
  const ready = v.ready === true;
  const ok = ready && pageErrors.length === 0;
  const said = readyText({ ready, ready_ms: count(v.ready_ms) });
  reading.viewports.push({ name, ready, ok, text: said });
  reading.failing ||= !ok;
  for (const e of pageErrors) note(reading.errors, name, e, (a, b) => a === b);
  for (const r of list(v.failed_requests)) {
    const request = failedRequest(r);
    if (!request) continue;
    note(reading.failedRequests, name, request, (a, b) => a.url === b.url && a.status === b.status);
  }
  reading.refusedWrites += list(v.refused_writes).length;
  for (const what of strings(v.refused_writes)) {
    if (!reading.refused.includes(what)) reading.refused.push(what);
  }
  for (const what of strings(v.refused_off_origin)) {
    if (!reading.offOrigin.includes(what)) reading.offOrigin.push(what);
  }
}

function screenReading(raw: unknown): ScreenReading | null {
  if (!record(raw) || typeof raw.path !== "string") return null;
  const reading: ScreenReading = {
    path: raw.path,
    name: screenName(raw.path),
    viewports: [],
    failing: false,
    errors: [],
    failedRequests: [],
    refusedWrites: 0,
    refused: [],
    offOrigin: []
  };
  for (const v of list(raw.viewports)) {
    if (record(v)) addViewport(reading, v);
  }
  return reading;
}

/** The screens of a `smoke` or `production_smoke` detail, failing ones first and
 *  otherwise in the runner's order. */
export function smokeScreens(detail: BuildCheckDetail | null | undefined): ScreenReading[] {
  const screens = list(detail?.screens)
    .map(screenReading)
    .filter((s): s is ScreenReading => s !== null);
  return [...screens.filter((s) => s.failing), ...screens.filter((s) => !s.failing)];
}

/** `14 screens`, `discovered` when the runner found them itself, and how many it left
 *  out. Empty when the detail names no screens. */
export function screensLine(detail: BuildCheckDetail | null | undefined, screens: number): string {
  if (screens === 0) return "";
  const parts = [`${screens} screen${screens === 1 ? "" : "s"}`];
  if (detail?.discovered === true) parts.push("discovered");
  const omitted = count(detail?.screens_omitted) ?? 0;
  if (omitted > 0) parts.push(`${omitted} left out`);
  return parts.join(" · ");
}

/** The screens of the app the opened ones link to that no check opened. */
export interface UnlistedReading {
  /** The ones the runner named (its first fifty), each as its row would show it. */
  names: string[];
  /** How many there are in all. */
  total: number;
}

/** What a `smoke` detail says was linked and not opened; null when it names none: every
 *  linked screen was in the list, or the runner is one from before it said. */
export function unlistedScreens(
  detail: BuildCheckDetail | null | undefined
): UnlistedReading | null {
  const names = strings(detail?.screens_unlisted).map(screenName);
  const total = count(detail?.screens_unlisted_total) ?? names.length;
  return total > 0 ? { names, total } : null;
}

/** The errors production shows now and did not before the move. */
export function newErrors(detail: BuildCheckDetail | null | undefined): BuildCheckNewError[] {
  return list(detail?.new_errors).flatMap((raw) => {
    if (!record(raw) || typeof raw.path !== "string") return [];
    const error = text(raw.error);
    return error ? [{ path: raw.path, viewport: text(raw.viewport) ?? "", error }] : [];
  });
}

/** What the platform did to production after a failed `production_smoke`. */
export interface RollbackReading {
  rolledBack: boolean;
  /** The build production went back to, as it was published. */
  to: string | null;
  /** Why production was left as it is. */
  reason: string | null;
  at: string | null;
}

/**
 * The automatic rollback a failed production check records; null when the app does
 * not roll back by itself, the follow-up has not run yet, or the server sends none.
 */
export function rollbackOf(detail: BuildCheckDetail | null | undefined): RollbackReading | null {
  const raw: unknown = detail?.rollback;
  if (!record(raw) || typeof raw.rolled_back !== "boolean") return null;
  const to = record(raw.to) ? (text(raw.to.build_id) ?? text(raw.to.id)) : null;
  return { rolledBack: raw.rolled_back, to, reason: text(raw.reason), at: text(raw.at) };
}

/** One screen at one viewport of `changes`, with the name its row shows. */
export interface ChangedReading extends BuildCheckChangedScreen {
  name: string;
  key: string;
}

function changedReading(raw: unknown): ChangedReading | null {
  if (!record(raw) || typeof raw.path !== "string") return null;
  const viewport = text(raw.viewport) ?? "";
  return {
    path: raw.path,
    viewport,
    changed: raw.changed === true,
    ratio: count(raw.ratio),
    staging: text(raw.staging),
    production: text(raw.production),
    diff: text(raw.diff),
    production_ready: typeof raw.production_ready === "boolean" ? raw.production_ready : null,
    production_errors: strings(raw.production_errors),
    name: screenName(raw.path),
    key: `${raw.path}@${viewport}`
  };
}

/** The screens of a `changes` detail, the changed ones first and otherwise in the
 *  runner's order. */
export function changedScreens(detail: BuildCheckDetail | null | undefined): ChangedReading[] {
  const screens = list(detail?.screens)
    .map(changedReading)
    .filter((s): s is ChangedReading => s !== null);
  return [...screens.filter((s) => s.changed), ...screens.filter((s) => !s.changed)];
}

/** How many screens differ, of how many compared: the detail's own counts, else what
 *  its rows come to. */
export function changesCount(
  detail: BuildCheckDetail | null | undefined,
  screens: readonly ChangedReading[]
): { changed: number; compared: number } {
  return {
    changed: count(detail?.changed) ?? screens.filter((s) => s.changed).length,
    compared: count(detail?.compared) ?? screens.length
  };
}

/** The share of pixels that differs: `3.1%`, `<0.1%`, or `differs` when the two
 *  pictures are not the same size and no share was measured. Empty for a screen that
 *  did not change. */
export function ratioText(screen: Pick<BuildCheckChangedScreen, "changed" | "ratio">): string {
  if (!screen.changed) return "";
  if (screen.ratio === null) return "differs";
  const percent = screen.ratio * 100;
  if (percent < 0.1) return "<0.1%";
  return `${percent >= 10 ? Math.round(percent) : percent.toFixed(1)}%`;
}

/**
 * Whether a `changes` row is waiting for a person: the app holds Promote until the
 * changed screens are seen, some changed, and nobody has said so. Its row then starts
 * open, so the pictures and the button are on screen without a click.
 */
export function awaitsALook(
  check: { kind: string; detail: BuildCheckDetail | null },
  seenRequired: boolean
): boolean {
  if (check.kind !== "changes" || !seenRequired) return false;
  const { changed } = changesCount(check.detail, changedScreens(check.detail));
  return changed > 0 && seenOf(check.detail) === null;
}

/** Who marked the changed screens seen, when someone has. */
export function seenOf(detail: BuildCheckDetail | null | undefined): BuildCheckSeen | null {
  const seen: unknown = detail?.seen;
  if (!record(seen)) return null;
  const by = text(seen.by);
  return by ? { by, at: text(seen.at) ?? "" } : null;
}

/** What "Mark as seen" was told. The 409 is an expected answer, not a fault. */
export function seenRefusal(error: unknown): string {
  if (apiStatus(error) === 409 && apiErrorCode(error) === "nothing_to_see") {
    return "Nothing changed.";
  }
  return apiErrorMessage(error, "Couldn't mark it seen.");
}
