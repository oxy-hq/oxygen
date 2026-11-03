// A browser check for selector materialization, run by showcase.yaml (the unit
// suite has no browser). Exits non-zero when any promise breaks:
//
// 1. Reading strategies must not invalidate the model's `aria-ref=` map. Any
//    aria snapshot replaces that map, so a strategy read that took one made
//    every ref the model was about to click resolve to nothing.
// 2. A form field named by its <label> or placeholder gets a durable `role=`
//    strategy, so typing into it replays.
// 3. A test id the page renders twice never outranks the model's scoped
//    selector, so replay does not click the other copy.
//
//   node --import tsx tests/agentic/runner/selectors.browser-check.ts

import { chromium } from "@playwright/test";
import { isNonDurableRecording, materializeStrategies } from "./selectors";

const failures: string[] = [];
const check = (ok: boolean, what: string) => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failures.push(what);
};

const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage();
  await page.setContent(
    '<label for=n>Name</label><input id=n><input placeholder="Search apps"><button>Create kiosk</button>'
  );
  const snap = await page.locator("body").ariaSnapshot({ mode: "ai" });
  const ref = (pattern: RegExp) => `aria-ref=${pattern.exec(snap)?.[1] ?? "missing"}`;
  const button = ref(/button "Create kiosk" \[ref=(\w+)\]/);
  const named = ref(/textbox "Name" \[ref=(\w+)\]/);
  const searched = ref(/textbox "Search apps" \[ref=(\w+)\]/);

  const namedStrategies = await materializeStrategies(page, "browser_type", { selector: named });
  const searchedStrategies = await materializeStrategies(page, "browser_type", {
    selector: searched
  });
  await materializeStrategies(page, "browser_click", { selector: button });

  check(
    (await page.locator(button).count()) === 1,
    "a ref still resolves after strategies are read"
  );
  check(
    !isNonDurableRecording(named, namedStrategies),
    "a <label>-named input records a durable strategy"
  );
  check(
    !isNonDurableRecording(searched, searchedStrategies),
    "a placeholder-named input records a durable strategy"
  );
  await page.locator(button).click({ timeout: 2_000 });
  check(true, "clicking by ref works after strategies are read");

  // Two composers share a test id; the model scoped its click to the one in
  // the dock. The recording must not rank the bare, ambiguous test id first.
  await page.setContent(
    '<div><button data-testid="send">Send</button></div>' +
      '<div data-testid="dock"><button data-testid="send">Send</button></div>'
  );
  const scoped = '[data-testid="dock"] [data-testid="send"]';
  const dockStrategies = await materializeStrategies(page, "browser_click", { selector: scoped });
  check(
    dockStrategies[0]?.selector === scoped,
    "a scoped selector stays rank 0 over its element's ambiguous test id"
  );
  check(
    !dockStrategies.some((s) => s.selector === '[data-testid="send"]'),
    "an ambiguous test id is not recorded as a strategy"
  );
} catch (err) {
  check(false, `the check itself threw: ${err instanceof Error ? err.message : String(err)}`);
} finally {
  await browser.close();
}
process.exit(failures.length === 0 ? 0 : 1);
