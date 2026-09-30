// What the planner cannot see from code: the start page's accessibility tree,
// signed in as the showcase identity, at the capture viewport. A corrective
// re-plan gets it, so its steps name controls that exist ("+ New kiosk"), not
// ones the diff suggested ("the Kiosks tab").

import { chromium } from "@playwright/test";
import { captureContextOptions, settle } from "../agentic/runner/capture-profile";
import { signIn } from "../agentic/runner/session";
import { VIEWPORT } from "./capture";
import type { Session } from "./session";

const BUDGET = 20_000;

export async function startPageTree(
  baseUrl: string,
  startPath: string,
  session: Session
): Promise<string> {
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({
      baseURL: baseUrl,
      ...captureContextOptions({
        dir: "",
        startPath,
        viewport: VIEWPORT,
        deviceScaleFactor: 1,
        slowMoMs: 0,
        video: false
      })
    });
    await signIn(context, baseUrl, session.token, session.user);
    const page = await context.newPage();
    await page.goto(startPath);
    await settle(page);
    const tree = await page.locator("body").ariaSnapshot();
    return tree.length > BUDGET
      ? `${tree.slice(0, BUDGET)}\n[truncated: ${tree.length - BUDGET} more characters not shown]`
      : tree;
  } finally {
    await browser.close();
  }
}
