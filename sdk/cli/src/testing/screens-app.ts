/**
 * A scripted app with three screens, as staging and as production serve it,
 * for the tests of the checks that open every screen. Test support only.
 *
 * The root links to `stores` and `reorder/plans` (and to things that are no
 * screen of the app: another site, an API route, a download). Staging serves
 * the build under test and production the one before it; `stores` is redrawn
 * on staging, so it is the one screen that differs.
 */

import { BUILD_ROW, type FakePlatform } from "./fake-checks-platform.js";
import { fakePng } from "./fake-page.js";
import type { PageScript } from "./fake-playwright.js";

/** The build production serves while the build under test is on staging, as it was published. */
export const PRODUCTION_BUILD = "60e2f19a";
/** That build's row id: what production's page reports as its build. */
export const PRODUCTION_ROW = "c3c3c3c3-2222-3333-4444-555555555555";
/** A page's text, which may be a customer's: it must reach the platform and nothing else. */
export const SECRET = "Acme payroll: 4 people owed 12,900";

const WHITE = [255, 255, 255, 255] as const;
const BLACK = [0, 0, 0, 255] as const;

/**
 * Which environment a context belongs to in a run that has only `smoke` and
 * `changes` open: contexts 0 and 1 are staging's desk and phone, 2 compares
 * pictures, 3 and 4 are production's desk and phone.
 */
export const onProduction = (context: number) => context >= 3;

/** What a claim adds for an attempt of an org that holds promotes on its checks. */
export const GATE_CLAIM = {
  open: ["smoke", "changes"],
  screens: null,
  production: { id: PRODUCTION_ROW, build_id: PRODUCTION_BUILD },
  baseline: null,
  limits: { screens: 30, file_bytes: 2_000_000, files: 200 }
};

/** The page script of the app on `platform`: what each context shows at each URL. */
export function screensApp(platform: FakePlatform): (context: number, url: string) => PageScript {
  return (context, url) => {
    const production = onProduction(context);
    const path = url.slice(platform.pageUrl.length);
    const redrawn = path === "stores" && !production;
    return {
      requests: [
        {
          method: "POST",
          url: `${platform.pageUrl}__oxy/beacon`,
          postData: '{"events":[{"n":"oxy-app-ready"}]}'
        }
      ],
      app: production
        ? { environment: "production", buildId: PRODUCTION_ROW }
        : { environment: "staging", buildId: BUILD_ROW },
      links: [
        `${platform.pageUrl}stores`,
        `${platform.pageUrl}reorder/plans?week=47`,
        `${platform.pageUrl}stores#top`,
        "https://elsewhere.example.com/pricing",
        `${platform.url}/api/orgs`,
        { href: `${platform.pageUrl}export.csv`, download: true }
      ],
      text: `${path || "home"} · ${SECRET}`,
      shot: fakePng(4, 4, WHITE, (x) => (redrawn && x < 2 ? BLACK : undefined))
    };
  };
}
