/**
 * A scripted page of an app on staging, loaded through the promote checks'
 * guarded browser path: what the tests of that path share. Test support only.
 */

import { observeAll } from "../checks/loads-browser.js";
import { type LoadsTarget, loadsTarget, VIEWPORTS } from "../checks/loads-target.js";
import { FakeChromium, type PageScript } from "./fake-playwright.js";

export const ORIGIN = "https://app.example.com";
export const BASE = `${ORIGIN}/customer-apps/acme/store/`;
export const APP = "00000000-0000-0000-0000-0000000000a1";
export const WS = "00000000-0000-0000-0000-0000000000b1";
/** The runtime's ready beacon, as the page sends it. */
export const BEACON = {
  method: "POST",
  url: `${BASE}__oxy/beacon`,
  postData: '{"v":1,"events":[{"n":"oxy-app-ready"}]}'
};
export const STAGING_APP = { environment: "staging", buildId: "b1", projectId: WS };
/** What a signed request carries on staging. */
export const KEY = { "x-api-key": "oxysvc_x", "x-oxy-app-env": "staging" };

export function target(overrides: Partial<LoadsTarget> = {}): LoadsTarget {
  return {
    ...loadsTarget({
      pageUrl: BASE,
      deployment: ORIGIN,
      credential: { "x-api-key": "oxysvc_x" },
      appId: APP,
      workspaceId: WS,
      environment: "staging",
      expectedBuild: "b1"
    }),
    // No test waits half a minute for a beacon that is not coming.
    waitMs: 40,
    ...overrides
  };
}

/** Load both viewports of a page that does the same thing in each. */
export async function observe(script: PageScript, t = target(), env: NodeJS.ProcessEnv = {}) {
  const chromium = new FakeChromium(() => script);
  const loads = await observeAll(t, VIEWPORTS, { load: async () => chromium, settleMs: 0, env });
  return { chromium, loads, desk: loads[0]?.[1], phone: loads[1]?.[1] };
}

/** What the first viewport's requests went out as: method, URL, what became of them. */
export function deskSent(chromium: FakeChromium) {
  return chromium.sent.filter((s) => s.context === 0);
}
