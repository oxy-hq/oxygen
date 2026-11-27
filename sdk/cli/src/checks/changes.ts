/**
 * `changes`: what a build changes on screen, against what production serves
 * now. Each screen at each viewport is pictured on staging and on production,
 * the two pictures are compared pixel by pixel (`pixel-compare.ts`), and the
 * evidence is kept ON THE PLATFORM (`PUT …/checks/files/{name}`): both
 * pictures, a diff picture when the screen changed, and for a changed screen
 * the visible text of both, which the platform's advisory notes read.
 *
 * IT INFORMS. The check passes whenever it ran; what it found is a count and
 * a list a person looks at. So a file that could not be kept (too large, the
 * attempt's file count used up, an upload that failed) is a WARNING in the
 * detail, never a failed check.
 *
 * NOTHING A PAGE SHOWED IS PRINTED OR WRITTEN TO DISK. A picture and a page's
 * text are a customer's data: held in memory until uploaded, then dropped.
 */

import { type Finding, finding, messageOf, type Outcome, outcome } from "./outcome.js";
import {
  CHANNEL_TOLERANCE,
  type CompareAnswer,
  compareInPage,
  installExpression,
  isChanged
} from "./pixel-compare.js";
import type { PwBrowser } from "./playwright.js";
import { BASELINE_MAX_ERRORS, type FileType, type Limits } from "./promote-api.js";
import { diffName, screenLabel, shotName, textName } from "./screens.js";
import type { ScreenVisit } from "./screens-browser.js";
import { errorsOf } from "./smoke-score.js";

/** Uploads that fail this many times in a row are not tried again. */
const MAX_FAILURES_IN_A_ROW = 3;

/** One screen at one viewport, compared. */
export interface ChangeRow {
  path: string;
  viewport: string;
  changed: boolean;
  /** The share of pixels that differ; 1 when one of the two pictures is missing. */
  ratio: number;
  /** The kept files' names; null for one that was not kept. */
  staging: string | null;
  production: string | null;
  diff: string | null;
  production_ready: boolean;
  production_errors: string[];
}

/**
 * The attempt's files, kept within its limits. `keep` never throws: a file
 * that is not kept is a warning, and says why.
 */
export class EvidenceFiles {
  private readonly problems: string[] = [];
  private left: number;
  private failures = 0;
  /** Why nothing more is uploaded, once that is so. */
  private stopped?: string;
  private skipped = 0;

  constructor(
    private readonly limits: Limits,
    private readonly put: (name: string, type: FileType, bytes: Uint8Array) => Promise<void>
  ) {
    this.left = limits.files;
  }

  warn(problem: string): void {
    this.problems.push(problem);
  }

  /** Every file that was not kept and why, the ones after a stop counted together. */
  warnings(): string[] {
    if (!this.stopped) return [...this.problems];
    const more = this.skipped > 0 ? ` (${this.skipped} more file(s) were not uploaded)` : "";
    return [...this.problems, `${this.stopped}${more}`];
  }

  async keep(name: string, type: FileType, bytes: Uint8Array): Promise<boolean> {
    if (this.stopped) {
      this.skipped += 1;
      return false;
    }
    if (bytes.byteLength > this.limits.file_bytes) {
      this.warn(
        `${name} is ${bytes.byteLength} bytes, more than the ${this.limits.file_bytes} a file may be: not uploaded`
      );
      return false;
    }
    if (this.left <= 0) {
      this.stopped = `the attempt's ${this.limits.files} files are used: ${name} was not uploaded`;
      return false;
    }
    // Taken before the request, given back when it fails: two viewports upload side by side.
    this.left -= 1;
    try {
      await this.put(name, type, bytes);
      this.failures = 0;
      return true;
    } catch (cause) {
      this.left += 1;
      this.failed(name, cause);
      return false;
    }
  }

  private failed(name: string, cause: unknown): void {
    const code = (cause as { serverCode?: unknown } | null)?.serverCode;
    if (code === "too_many_files" || code === "attempt_closed") {
      const why = code === "too_many_files" ? "holds all the files it may" : "is closed";
      this.stopped = `the attempt ${why}: ${name} was not uploaded`;
      return;
    }
    this.failures += 1;
    this.warn(`${name} could not be uploaded: ${messageOf(cause)}`);
    if (this.failures >= MAX_FAILURES_IN_A_ROW) {
      this.stopped = `${MAX_FAILURES_IN_A_ROW} uploads failed in a row: no more were tried`;
    }
  }
}

/** Compare two pictures (PNG bytes). */
export type Compare = (a: Uint8Array, b: Uint8Array) => Promise<CompareAnswer>;

/**
 * A blank page of the check's browser that compares pictures. It loads
 * nothing, and anything it asked for would not be sent.
 */
export async function pictureComparer(
  browser: PwBrowser
): Promise<{ compare: Compare; close: () => Promise<void> }> {
  const context = await browser.newContext({ serviceWorkers: "block" });
  await context.route("**/*", (route) => route.abort("blockedbyclient").catch(() => undefined));
  const page = await context.newPage();
  await page.evaluate<void>(installExpression());
  const base64 = (bytes: Uint8Array) => Buffer.from(bytes).toString("base64");
  return {
    compare: (a, b) =>
      page.evaluate(compareInPage, {
        a: base64(a),
        b: base64(b),
        tolerance: CHANNEL_TOLERANCE
      }),
    close: () => context.close().catch(() => undefined)
  };
}

/** One screen's comparison: the pictures, where they are kept, and what to tell a person. */
export interface Comparison {
  staging: ScreenVisit;
  production: ScreenVisit;
  /** The screen's slug, for its files' names. */
  slug: string;
  files: EvidenceFiles;
  compare: Compare;
}

/**
 * Compare one screen's staging picture with its production one and keep the
 * evidence. Both visits' pictures and text are dropped before this returns.
 */
export async function compareScreen(input: Comparison): Promise<ChangeRow> {
  const { staging, production, slug, files } = input;
  const viewport = production.viewport.name;
  const row: ChangeRow = {
    path: production.path,
    viewport,
    changed: false,
    ratio: 0,
    staging: null,
    production: null,
    diff: null,
    production_ready: production.observed.readyMs !== null,
    // What a later `production_smoke` is told was already there. A list cut here is one that
    // check takes as cut: an error it does not hold is then not called new.
    production_errors: errorsOf(production.observed).slice(0, BASELINE_MAX_ERRORS)
  };
  row.staging = await keptPicture(files, shotName("staging", slug, viewport), staging.shot);
  row.production = await keptPicture(
    files,
    shotName("production", slug, viewport),
    production.shot
  );
  await comparePictures(row, input);
  if (row.changed) await keepTexts(input);
  // On the platform now, or not to be: nothing here holds a page's picture or text longer.
  for (const visit of [staging, production]) {
    visit.shot = undefined;
    visit.text = undefined;
  }
  return row;
}

/** Keep a picture when there is one: its name once it is kept, null when it was not. */
async function keptPicture(
  files: EvidenceFiles,
  name: string,
  bytes: Uint8Array | undefined
): Promise<string | null> {
  return bytes !== undefined && (await files.keep(name, "image/png", bytes)) ? name : null;
}

/**
 * Fill the row's ratio, verdict and diff picture from the two pictures. A pair
 * that is not whole, or cannot be compared, counts as changed.
 */
async function comparePictures(row: ChangeRow, input: Comparison): Promise<void> {
  const { staging, production, slug, files } = input;
  const viewport = production.viewport.name;
  const where = `${screenLabel(production.path)} at ${viewport}`;
  const [a, b] = [staging.shot, production.shot];
  if (!a || !b) {
    missing(row, files, `${where}: no picture of ${a ? "production" : "staging"} could be taken`);
    return;
  }
  try {
    const found = await input.compare(a, b);
    row.ratio = Math.round(found.ratio * 10_000) / 10_000;
    row.changed = isChanged(found);
    const diff = row.changed && found.diff ? Buffer.from(found.diff, "base64") : undefined;
    row.diff = await keptPicture(files, diffName(slug, viewport), diff);
  } catch (cause) {
    missing(row, files, `${where}: the pictures could not be compared (${messageOf(cause)})`);
  }
}

/** A changed screen's visible text on both environments, which the platform's advisory notes read. */
async function keepTexts(input: Comparison): Promise<void> {
  const { staging, production, slug, files } = input;
  const viewport = production.viewport.name;
  for (const [environment, visit] of [
    ["staging", staging],
    ["production", production]
  ] as const) {
    if (visit.text === undefined) continue;
    const text = Buffer.from(JSON.stringify({ text: visit.text }), "utf8");
    await files.keep(textName(environment, slug, viewport), "application/json", text);
  }
}

/** A screen that could not be compared counts as changed, so a person looks at it. */
function missing(row: ChangeRow, files: EvidenceFiles, why: string): void {
  row.changed = true;
  row.ratio = 1;
  files.warn(`${why}, so the screen counts as changed`);
}

/** `4 of 28 screens differ`. */
function differ(changed: number, compared: number): string {
  return `${changed} of ${compared} screen${compared === 1 ? "" : "s"} ${changed === 1 ? "differs" : "differ"}`;
}

/** The check's outcome over every compared screen. `uncompared` were not reached in time. */
export function scoreChanges(
  productionBuild: string,
  rows: readonly ChangeRow[],
  warnings: readonly string[],
  uncompared: number
): Outcome {
  const changed = rows.filter((row) => row.changed).length;
  const findings: Finding[] = warnings.map((warning) => finding("warning", warning));
  if (uncompared > 0) {
    findings.unshift(
      finding("warning", `${uncompared} screen(s) were not compared before the time was up`)
    );
  }
  const detail = {
    production_build_id: productionBuild,
    compared: rows.length,
    changed,
    screens: rows,
    seen: null
  };
  if (rows.length === 0) {
    return outcome("error", "no screen could be compared before the time was up", findings, detail);
  }
  return outcome("passed", differ(changed, rows.length), findings, detail);
}

/** Production serves nothing, or this same build: there is nothing to compare with. */
export function skippedChanges(warnings: readonly string[] = []): Outcome {
  return outcome(
    "skipped",
    "nothing to compare: production serves no other build",
    warnings.map((warning) => finding("warning", warning)),
    { production_build_id: null, compared: 0, changed: 0, screens: [], seen: null }
  );
}
