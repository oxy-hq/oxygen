import assert from "node:assert/strict";
import { test } from "node:test";
import { renderSummary } from "../../roadmap/doc.ts";
import { parseTrailer } from "../../roadmap/trailer.ts";
import { ROADMAP } from "../config.ts";
import type { RoadmapInput } from "./roadmap.ts";
import { evaluateRoadmap, productFiles } from "./roadmap.ts";

const doc = `# Platform roadmap

## 2. By audience

### 2.1 Customers use apps

- **Now**
  - **Open apps pick up a new version.** Tabs notice ([#3620]).
    <!-- rm:open-apps -->
- **Next**
  - **Phone apps in the app stores.** Mobile moves toward the stores.
    <!-- rm:phone-apps -->
- **Later**

<!-- PR links -->
[#3620]: https://github.com/oxy-hq/oxygen-internal/pull/3620
`;
const summary = renderSummary(doc);
const run = (o: Partial<RoadmapInput> & { body?: string | null }) =>
  evaluateRoadmap({
    files: ["crates/app/src/server/api/x.rs"],
    trailer: parseTrailer(o.body ?? null),
    pr: 3620,
    draft: false,
    doc,
    summary,
    config: ROADMAP,
    ...o
  });

test("roadmap status: which files count as product code", () => {
  const product = productFiles(
    [
      "crates/app/src/server/api/x.rs",
      "crates/app/tests/platform/x.rs",
      "crates/auth/src/token/device_login_tests.rs",
      "web-app/src/pages/Home.tsx",
      "web-app/src/pages/Home.test.tsx",
      "sdk/cli/src/main.ts",
      "crates/workspace-hack/Cargo.toml",
      "crates/app/Cargo.toml",
      "internal-docs/x.md",
      ".github/workflows/ci.yaml"
    ],
    ROADMAP
  );
  assert.deepEqual(product, [
    "crates/app/src/server/api/x.rs",
    "web-app/src/pages/Home.tsx",
    "sdk/cli/src/main.ts"
  ]);
});

test("roadmap status: no trailer", () => {
  assert.equal(run({}).state, "failure");
  assert.match(run({}).description, /no roadmap trailer/);
  assert.equal(run({ files: ["internal-docs/x.md", "crates/app/tests/y.rs"] }).state, "success");
  assert.equal(
    run({ files: [ROADMAP.doc] }).state,
    "failure",
    "a roadmap edit of unknown shape names its line"
  );
  assert.equal(run({ draft: true }).state, "success");
});

test("roadmap status: none", () => {
  const none = "Roadmap: none -- a bug fix with no roadmap line";
  assert.equal(run({ body: none }).state, "success");
  assert.match(run({ body: none }).description, /none \(a bug fix/);
  assert.equal(
    run({ body: none, files: [ROADMAP.doc, "crates/x/src/a.rs"] }).state,
    "failure",
    "a roadmap edit of unknown shape is a move"
  );
  assert.equal(run({ body: "Roadmap: none" }).state, "failure");
});

test("roadmap status: named lines must exist and cite the PR", () => {
  assert.equal(run({ body: "Roadmap: open-apps" }).state, "success");
  assert.match(run({ body: "Roadmap: nope" }).description, /No roadmap line nope/);
  const silent = run({ body: "Roadmap: phone-apps" });
  assert.equal(silent.state, "failure");
  assert.match(silent.description, /does not cite #3620/);
  assert.equal(run({ body: "Roadmap: phone-apps", pr: null }).state, "success", "no PR number yet");
  assert.equal(run({ body: "Roadmap: open-apps", doc: null }).state, "failure");
});

test("roadmap status: a mechanical roadmap edit (pins, wrapping, prose) moves no line", () => {
  const before = doc
    .replace(/\n {4}<!-- rm:[a-z-]+ -->/g, "")
    .replace("# Platform roadmap", "# Platform roadmap\n\nOld prose.");
  const files = [ROADMAP.doc, ROADMAP.summary];
  assert.equal(run({ files, docBefore: before }).state, "success", "no trailer needed");
  assert.equal(
    run({ files, docBefore: before, body: "Roadmap: none -- pins only" }).state,
    "success"
  );
  const moved = before.replace("Tabs notice", "Tabs now notice");
  assert.equal(run({ files, docBefore: moved }).state, "failure", "a reworded line is a move");
  assert.equal(
    run({ files, docBefore: moved, body: "Roadmap: none -- pins only" }).state,
    "failure"
  );
  assert.equal(run({ files, docBefore: null }).state, "failure", "unknown before: assume a move");
});

test("roadmap status: a changed roadmap has every line pinned", () => {
  const files = [ROADMAP.doc, ROADMAP.summary];
  const unpinned = doc.replace("    <!-- rm:phone-apps -->\n", "");
  const result = run({ files, doc: unpinned, summary: renderSummary(unpinned), docBefore: doc });
  assert.equal(result.state, "failure");
  assert.match(result.description, /1 roadmap line\(s\) have no .* pin --write/);
  assert.equal(
    run({ doc: unpinned, body: "Roadmap: none -- a bug fix only" }).state,
    "success",
    "an unchanged roadmap is not re-checked"
  );
});

test("roadmap status: the summary stays derived when the roadmap changes", () => {
  const files = [ROADMAP.doc, "crates/app/src/server/api/x.rs"];
  assert.equal(run({ body: "Roadmap: open-apps", files }).state, "success");
  const stale = run({ body: "Roadmap: open-apps", files, summary: "# old" });
  assert.equal(stale.state, "failure");
  assert.match(stale.description, /summary --write/);
  assert.equal(
    run({ body: "Roadmap: none -- x y z w", files, summary: "# old" }).state,
    "failure",
    "whatever the trailer says"
  );
  assert.equal(
    run({ body: "Roadmap: open-apps", summary: "# old" }).state,
    "success",
    "an unrelated PR is not blamed for a stale summary"
  );
  assert.ok(stale.description.length <= 140);
});
