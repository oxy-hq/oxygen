// A fixed browser profile for runs whose output is media, not a verdict.
//
// The release showcase (tests/showcase/) records what a feature looks like, and
// every capture must look like every other one: same window, same theme, same
// locale and clock zone, motion reduced, and the page settled before anything
// is framed. Tests never set this — without a profile the runtime keeps
// Playwright's defaults, exactly as before.

import { renameSync } from "node:fs";
import { join } from "node:path";
import type { BrowserContext, BrowserContextOptions, Page } from "@playwright/test";

export interface CaptureProfile {
  /** Directory the screenshot and video are written to. */
  dir: string;
  /** Opened and settled before the steps run; video before it is trimmed. */
  startPath: string;
  viewport: { width: number; height: number };
  deviceScaleFactor: number;
  /** Pause Playwright adds between operations, so a replay is watchable. */
  slowMoMs: number;
  video: boolean;
  /** CSS applied only while the final screenshot is taken. */
  screenshotStyle?: string;
}

const SCREENSHOT_FILE = "screenshot.png";
const VIDEO_FILE = "video.webm";

export function captureContextOptions(profile: CaptureProfile): BrowserContextOptions {
  return {
    viewport: profile.viewport,
    deviceScaleFactor: profile.deviceScaleFactor,
    colorScheme: "light",
    reducedMotion: "reduce",
    locale: "en-US",
    timezoneId: "UTC",
    recordVideo: profile.video ? { dir: profile.dir, size: profile.viewport } : undefined
  };
}

/**
 * Wait until the page has stopped changing. Each wait is bounded and
 * best-effort: a page that keeps a socket open never reaches `networkidle`,
 * and that must cost a few seconds, not the capture.
 */
export async function settle(page: Page): Promise<void> {
  await page.waitForLoadState("networkidle", { timeout: 15_000 }).catch(() => undefined);
  // Strings, not closures: this tooling compiles without the DOM lib.
  await page.evaluate("document.fonts.ready.then(() => undefined)").catch(() => undefined);
  await page
    .waitForFunction("!document.querySelector('[aria-busy=\"true\"]')", undefined, {
      timeout: 10_000
    })
    .catch(() => undefined);
  await page.waitForTimeout(500);
}

/** Navigate to the start page and return how long recording ran before it settled. */
export async function openStartPage(
  page: Page,
  profile: CaptureProfile,
  recordingSince: number
): Promise<number> {
  await page.goto(profile.startPath);
  await settle(page);
  return Date.now() - recordingSince;
}

export async function takeFinalScreenshot(page: Page, profile: CaptureProfile): Promise<string> {
  await settle(page);
  const path = join(profile.dir, SCREENSHOT_FILE);
  await page.screenshot({
    path,
    animations: "disabled",
    caret: "hide",
    style: profile.screenshotStyle
  });
  return path;
}

/**
 * Close the context — which is what finalizes a Playwright video — and move
 * the video to its fixed name. Returns undefined when no video was recorded.
 */
export async function closeAndCollectVideo(
  context: BrowserContext,
  page: Page,
  profile: CaptureProfile
): Promise<string | undefined> {
  const video = profile.video ? page.video() : null;
  await context.close();
  if (!video) return undefined;
  const recorded = await video.path();
  const dest = join(profile.dir, VIDEO_FILE);
  renameSync(recorded, dest);
  return dest;
}
