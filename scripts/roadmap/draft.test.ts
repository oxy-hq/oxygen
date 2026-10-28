import assert from "node:assert/strict";
import { test } from "node:test";
import { parseRoadmap } from "./doc.ts";
import { rankLines, renderDraft, words } from "./draft.ts";

const doc = parseRoadmap(`# Platform roadmap

## 2. By audience

### 2.1 Customers use apps

- **Now**
  - **Open apps pick up a new version.** Tabs notice a publish ([#3620]).
    <!-- rm:open-apps -->
  - Landed: **Kiosk enrolment by PIN.** Crew enrol a kiosk with a PIN ([#3500]).
    <!-- rm:kiosk-pin -->
- **Next**
  - **Phone apps in the app stores.** Mobile moves toward the stores.
    <!-- rm:phone-apps -->
- **Later**
`);

test("words: lower-case, four letters up, hyphens split, stopwords and types dropped", () => {
  assert.deepEqual(
    [...words("feat: Open-apps pick up a NEW version in crates/app/src/publish.rs")].sort(),
    ["apps", "open", "pick", "publish", "version"]
  );
});

test("rankLines: most shared words first, landed last on a tie, silent lines left out", () => {
  const ranked = rankLines(doc, [
    "feat: tabs notice a publish of a new version",
    "web-app/src/publish/notice.tsx"
  ]);
  assert.deepEqual(
    ranked.map((r) => [r.item.id, r.shared]),
    [["open-apps", ["notice", "publish", "tabs", "version"]]]
  );
  const tie = rankLines(doc, ["kiosk apps"]);
  assert.deepEqual(
    tie.map((r) => r.item.id),
    ["open-apps", "phone-apps", "kiosk-pin"]
  );
});

test("renderDraft: the verdict, the product files, the closest lines and the next step", () => {
  const text = renderDraft({
    title: "feat: tabs notice a publish",
    files: ["web-app/src/publish/notice.tsx", "web-app/src/publish/notice.test.tsx"],
    product: ["web-app/src/publish/notice.tsx"],
    verdict: {
      state: "failure",
      description: "1 product file(s) changed with no roadmap trailer."
    },
    ranked: rankLines(doc, ["feat: tabs notice a publish"])
  });
  assert.match(text, /^Title: feat: tabs notice a publish/);
  assert.match(text, /Product files \(1 of 2 changed\): web-app\/src\/publish\/notice\.tsx/);
  assert.match(text, /status says: failure · 1 product file/);
  assert.match(
    text,
    /open-apps {2}\(now\) {2}Open apps pick up a new version\. {2}\[notice, publish, tabs\]/
  );
  assert.match(text, /Roadmap: none -- <why>/);
  const none = renderDraft({
    title: "chore: tidy",
    files: ["crates/app/tests/x.rs"],
    product: [],
    verdict: { state: "success", description: "No product code changed" },
    ranked: []
  });
  assert.match(none, /Product files: none of the 1 changed files count/);
  assert.match(none, /No roadmap line shares a word with this branch/);
});
