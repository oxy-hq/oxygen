import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  allItems,
  derivedId,
  findItem,
  headline,
  linkPr,
  parseRoadmap,
  pinAll,
  pinnedId,
  renderSummary,
  stripPin,
  wrapItem
} from "./doc.ts";

const here = new URL(".", import.meta.url);
const real = readFileSync(new URL("../../internal-docs/platform-roadmap.md", here), "utf8");

const small = `# Platform roadmap

## 1. North star and scoreboard

**Oxygen is where apps run.** We win when more people use more apps.

Today's values are from the 2026-10-17 survey; cost re-read on 2026-10-22.
**Directions, not targets**: no target yet.

| Metric | Today | Direction | Source |
| --- | --- | --- | --- |
| **Apps live.** Apps live (published / registered) | 47 / 63 (of which 2 quiet) | up | arch §1 |

## 2. By audience

### 2.1 Customers use apps

Blurb.

- **Now**
  - **App updates go live safely.** Promoting a build is safe ([#3639]). More words that
    wrap onto a second line ([#3734], [#3732]).
  - Landed: **Open apps pick up a new version.** Tabs notice ([#3620]).
    <!-- rm:open-apps -->
- **Next**
  - **Phone apps in the app stores.** Mobile moves toward the stores.
- **Later**

### 2.2 Engineers and agents ship apps

- **Now**
- **Next**
  - **Engineers start using their private test copies of apps.** Sandboxes get used.
- **Later**

### 2.3 The platform scales

- **Now**
- **Next**
- **Later**
  - **Stop relying on the old editor.** Retire it.

## 3. Anti-goals

| We will not | Decided in |
| --- | --- |
| **Let customers build their own apps; Oxy builds them.** No builder. | somewhere |

## 4. Governance

Text.

<!-- PR links -->
[#3620]: https://github.com/oxy-hq/oxygen-internal/pull/3620
[#3639]: https://github.com/oxy-hq/oxygen-internal/pull/3639
[#3732]: https://github.com/oxy-hq/oxygen-internal/pull/3732
[#3734]: https://github.com/oxy-hq/oxygen-internal/pull/3734
`;

test("parses lines, continuations, pins, landed and PR refs", () => {
  const doc = parseRoadmap(small);
  const items = allItems(doc);
  assert.deepEqual(
    items.map((i) => [i.audience, i.horizon, i.id]),
    [
      ["customers", "now", "app-updates-go-live-safely"],
      ["customers", "now", "open-apps"],
      ["customers", "next", "phone-apps-in-the-app-stores"],
      ["engineers", "next", "engineers-start-using-their-private-test"],
      ["platform", "later", "stop-relying-on-the-old-editor"]
    ]
  );
  const first = items[0]!;
  assert.deepEqual(first.prs, [3639, 3734, 3732]);
  assert.equal(first.lineEnd - first.lineStart, 1, "two source lines");
  assert.equal(items[1]?.landed, true);
  assert.equal(items[1]?.prs[0], 3620);
  assert.equal(doc.scoreboard.length, 1);
  assert.equal(doc.antiGoals.length, 1);
  assert.equal(doc.prLinks[3620], "https://github.com/oxy-hq/oxygen-internal/pull/3620");
});

test("ids drop apostrophes and markup, and a pin wins", () => {
  assert.equal(
    derivedId("**Each app's progress, from first draft to live.** More"),
    "each-apps-progress-from-first-draft"
  );
  assert.equal(
    derivedId('Landed: **One clear answer to "is this app working?"** x'),
    "one-clear-answer"
  );
  assert.equal(
    derivedId("**Move between apps without leaving the one you are in.**"),
    "move-between-apps-without-leaving"
  );
  assert.equal(
    derivedId("plain words with no headline at all here"),
    "plain-words-with-no-headline"
  );
  assert.equal(pinnedId("Text <!-- rm:My-Id -->"), "my-id");
  assert.equal(stripPin("Text <!-- rm:a --> more <!-- rm:b -->"), "Text  more", "every pin goes");
  const nested = stripPin("Text <!-- rm:<!-- rm:x --> -->");
  assert.equal(pinnedId(nested), null, "a nested pin leaves no pin behind");
  assert.ok(!nested.includes("rm:x"));
});

test("wrapItem keeps the pin alone on the last line", () => {
  const lines = wrapItem(`${"word ".repeat(40).trim()} <!-- rm:some-id -->`);
  assert.equal(lines.at(-1), "    <!-- rm:some-id -->");
  assert.ok(lines.every((l) => l.length <= 92 || l.split(" ").length <= 2));
  assert.ok(lines[0]?.startsWith("  - word"));
});

test("pinAll pins every unpinned line without rewrapping, is idempotent; ids survive a rewording", () => {
  const pinned = pinAll(small);
  assert.equal(
    pinned.split("\n").length,
    small.split("\n").length + 4,
    "one pin line per unpinned item"
  );
  assert.ok(
    pinned.includes("More words that\n    wrap onto a second line"),
    "the author's wrapping stays"
  );
  const ids = allItems(parseRoadmap(pinned)).map((i) => i.id);
  assert.deepEqual(
    ids,
    allItems(parseRoadmap(small)).map((i) => i.id)
  );
  assert.equal(pinAll(pinned), pinned);
  const reworded = pinned.replace("**App updates go live safely.**", "**Releases are safe.**");
  assert.ok(findItem(parseRoadmap(reworded), "app-updates-go-live-safely"));
});

test("linkPr cites the PR on the line and adds its link definition, once", () => {
  const once = linkPr(small, "phone-apps-in-the-app-stores", 4001, "oxy-hq/oxygen-internal");
  const item = findItem(parseRoadmap(once), "phone-apps-in-the-app-stores")!;
  assert.deepEqual(item.prs, [4001]);
  assert.match(once, /^\[#4001\]: https:\/\/github\.com\/oxy-hq\/oxygen-internal\/pull\/4001$/m);
  assert.equal(linkPr(once, "phone-apps-in-the-app-stores", 4001, "x"), once);
  // A line that already ends in a PR list gets the new one inside it, keeping its pin.
  const listed = linkPr(small, "open-apps", 4002, "x");
  const open = findItem(parseRoadmap(listed), "open-apps")!;
  assert.deepEqual(open.prs, [3620, 4002]);
  assert.match(open.text, /\(\[#3620\], \[#4002\]\)/);
  assert.equal(pinnedId(open.text), "open-apps");
  assert.throws(() => linkPr(small, "nope", 1, "x"), /no roadmap line/);
});

test("the one-page summary carries headlines, Done, dates and no PR refs", () => {
  const out = renderSummary(small);
  assert.match(out, /read on 2026-10-17 and 2026-10-22/);
  assert.match(out, /^\| Apps live \| 47 \/ 63 \| up \|$/m);
  assert.match(out, /^ {2}- App updates go live safely\.$/m);
  assert.match(out, /^ {2}- Open apps pick up a new version\. Done\.$/m);
  assert.match(out, /^- Let customers build their own apps; Oxy builds them\.$/m);
  assert.doesNotMatch(out, /#\d{4}|rm:/);
  assert.equal(headline("Landed: plain words ([#1])."), "plain words. Done.");
});

test("the real roadmap parses to distinct pinned ids, and its committed summary is derived", () => {
  const items = allItems(parseRoadmap(real));
  assert.ok(items.length >= 20, `only ${items.length} lines`);
  assert.equal(new Set(items.map((i) => i.id)).size, items.length);
  assert.ok(items.every((i) => i.text.length > 10 && !/^\s/.test(i.text)));
  const unpinned = items.filter((i) => !pinnedId(i.text)).map((i) => i.id);
  assert.deepEqual(unpinned, [], "run: node scripts/roadmap/roadmap.ts pin --write");
  const committed = readFileSync(
    new URL("../../internal-docs/platform-roadmap-summary.md", here),
    "utf8"
  );
  assert.equal(
    renderSummary(real),
    committed,
    "run: node scripts/roadmap/roadmap.ts summary --write"
  );
});
