#!/usr/bin/env node
/**
 * Check that every package the `sdk` job SELECTS declares the scripts it runs.
 *
 * SELECTION IS BY RULE; EXECUTION IS STILL OPT-IN. `Typecheck`, `Test` and
 * `Build` all select with `--filter "$SDK_FILTER"`. But `pnpm run <script>` across
 * a multi-project selection runs the script in each project that HAS it and
 * errors only when none of them does — measured on a scratch workspace:
 *
 *     pkgs/has typecheck$ echo RAN-has     <- ran
 *     (pkgs/lacks: no typecheck)           <- skipped, no output, exit 0
 *
 * So a package that lands without a `typecheck` script is selected, silently
 * skipped, and the blocking step goes green having read nothing new — invisible
 * from the step's own output, which is why it needs a check and not a comment.
 *
 * THE SET COMES FROM PNPM, NOT FROM THE FILESYSTEM, and asking rather than
 * walking is the point. `./sdk/**` selects declared WORKSPACE PROJECTS, and
 * `pnpm-workspace.yaml` names its members individually — there is no `sdk/*`
 * glob there. A directory walk diverges both ways: it fails on a
 * `sdk/<dir>/package.json` that is not a member (selected by nothing, so the
 * failure would assert a cause that does not exist), and it misses a nested
 * member like `sdk/<group>/<pkg>`, which `./sdk/**` does select — the exact gap
 * this script is for.
 *
 * `--fail-if-no-match` on the steps covers the remaining direction: a filter
 * that matches nothing at all. The filter itself comes from `SDK_FILTER`, set
 * by the step, so this audits the set the steps select rather than a literal
 * that has to be kept in step with three others.
 */

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/**
 * Report and exit. EVERY failure in this script goes through here, including
 * the one it exists for — a package missing a script — so the shape of a
 * failure does not depend on which kind it is.
 *
 * @returns {never} so `return fail(…)` reads as terminal at the call site;
 * without it, two `fail()`s in sequence are correct only because of a
 * `process.exit` the signature does not mention. DOCUMENTATION, not a check —
 * no tsconfig includes `scripts/`, so a `return fail(…)` that stopped being
 * terminal would be caught by no tool in this repo.
 */
function fail(message, detail) {
  console.error(message);
  if (detail) console.error(`\n  ${detail}`);
  process.exit(1);
}

/**
 * The filter the `sdk` job selects with — PASSED IN, so this checks the set the
 * steps check.
 *
 * It was written here as a literal and in three `run:` lines, and the script
 * could not see the other three: narrow the job's filter and this gate would
 * quietly audit a different set than the steps it is about, which is the
 * two-lists failure the job's own comment is against, one level up. The default
 * keeps `node scripts/ci/verify-sdk-scripts.mjs` working by hand.
 */
// `||`, not `??`: an env set to the empty string is a job whose `env:` lost
// its value, not a caller asking for the empty filter. `??` would pass
// `--filter ""` and leave the root check below to catch it.

const DEFAULT_FILTER = "./sdk/**";
// `noUndeclaredEnvVars` is about turbo's cache keys. This script is invoked by
// `node` straight from a workflow step, never through a turbo task, so there is
// no cache for an undeclared var to poison — and adding it to `turbo.json`
// would assert a relationship that does not exist.
// Measured: removing this line produces the warning and restoring it silences
// it with no unused-suppression diagnostic, so the rule path is real in the
// Biome this repo pins — worth recording, because the obvious reading is that
// `noUndeclaredEnvVars` is turbo's ESLint rule and not one Biome has.
// biome-ignore lint/suspicious/noUndeclaredEnvVars: not a turbo task; see above
const FILTER = process.env.SDK_FILTER || DEFAULT_FILTER;

/**
 * The scripts the `sdk` job runs across the filter — READ OUT OF THE WORKFLOW.
 *
 * This was a hand-written `["typecheck", "test", "build"]`, coupled to
 * `ci.yaml` by nothing: the commit that shared the package LIST left the script
 * list unshared, so a fourth `--filter "$SDK_FILTER"` step would have gone
 * unchecked and dropping `Test` would have left the gate demanding a `test`
 * script nothing runs. Both are the two-lists failure this job argues against.
 *
 * `$SDK_FILTER` is the marker — the same variable that makes the selection one
 * list makes the script list derivable, because a step that selects with it is
 * exactly a step this gate is about. `sdk/typescript`'s advisory step does not
 * use it and is excluded from `Typecheck` by name, which is why that package is
 * still required to DECLARE `typecheck`: the carve-out changes which step runs
 * it, never whether it exists.
 */
function scriptsTheJobRuns() {
  const workflowPath = join(ROOT, ".github", "workflows", "ci.yaml");
  let workflow;
  try {
    workflow = readFileSync(workflowPath, "utf8");
  } catch (cause) {
    // The one arm `selected()` above does not cover, in the file whose point is
    // that a tool-or-filesystem condition never arrives unlabelled.
    return fail(
      "could not read the workflow this gate derives its requirements from",
      `${workflowPath}: ${cause.message}`
    );
  }

  // Join shell continuations first: `Typecheck` spreads its invocation over
  // four lines, so `$SDK_FILTER` and the script name are not on the same one.
  // A line-at-a-time scan found `test` and `build` and silently missed
  // `typecheck` — the gate then required less than the job runs, which is the
  // failure this derivation exists to prevent, in the derivation.
  const joined = workflow.replace(/\\\n\s*/g, " ");

  /** @type {Set<string>} */
  const scripts = new Set();
  let matched = 0;
  for (const line of joined.split("\n")) {
    // COMMENTS ARE PROSE, on both doors. `ci.yaml`'s comments discuss this
    // literal, and the loop otherwise contributes the last word of any line
    // containing it — so `# every step selects with --filter "$SDK_FILTER"
    // today.` made the blocking gate demand a `today.` script of every package.
    // Reproduced before fixing. A leading `#` is the standalone case; stripping
    // from the first ` #` is the trailing one, which a `run:` line is at least
    // as likely to carry.
    if (line.trim().startsWith("#")) continue;
    if (!line.includes('"$SDK_FILTER"')) continue;

    matched += 1;
    const command = line.split(" #")[0].trim();
    const words = command.split(/\s+/);
    const last = words[words.length - 1];
    if (last && !last.startsWith("-") && !last.includes("$") && !last.includes('"')) {
      scripts.add(last);
    }
  }

  // A MATCHED LINE THAT YIELDS NOTHING IS A PARSER MISS, not a step without a
  // script — and it fails the same way the line-at-a-time scan did: the gate
  // requires LESS than the job runs, silently. `pnpm --filter "$SDK_FILTER"
  // test -- --coverage` ends on a flag, is rejected by the guards above, and
  // contributes nothing. `REQUIRED.length === 0` cannot see it, because the
  // other two lines still contribute.
  if (scripts.size < matched) {
    fail(
      `read ${matched} step(s) selecting with $SDK_FILTER and could name only ${scripts.size} script(s)`,
      "the last-word rule is a heuristic over YAML, not a parse — an invocation that does not\n  " +
        "end on its script name (trailing flags, `-- args`) needs this function taught about it"
    );
  }
  return [...scripts].sort();
}

const REQUIRED = scriptsTheJobRuns();
if (REQUIRED.length === 0) {
  fail(
    "found no scripts run across `$SDK_FILTER` in .github/workflows/ci.yaml",
    "the gate derives what it requires from those steps — has the job been rewritten?"
  );
}

/**
 * Exactly what `--filter "$SDK_FILTER"` resolves to, asked of pnpm.
 *
 * EVERY WAY THIS CAN FAIL IS LABELLED, because an unhandled throw here is a
 * node stack where the job needed a sentence — the same "a tool condition
 * arriving unlabelled" class this repo's `oxyc validate` was fixed for.
 * Measured, so the arms match reality rather than a guess:
 *
 *     pnpm ls … --filter "./sdk/nope"                      exit 0, stdout `[]`
 *     pnpm ls … --filter "./sdk/nope" --fail-if-no-match    exit 1, stdout ``
 *     execFileSync("pnpm", …) with pnpm off PATH           throws ENOENT
 *
 * NOTE THE FIRST LINE: an empty selection is a clean `[]`, so the count check
 * below is reachable and gives the better message. `--fail-if-no-match` is
 * therefore NOT passed here — on the CI steps it turns a silent skip into a
 * failure, but on this `ls` it would turn a printable answer into an
 * exception. The flag is right there and wrong here.
 */
function selected() {
  let out;
  try {
    out = execFileSync("pnpm", ["ls", "-r", "--depth", "-1", "--json", "--filter", FILTER], {
      cwd: ROOT,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "inherit"]
    });
  } catch (cause) {
    if (cause.code === "ENOENT") {
      // ONE REMEDY, because the two causes have one fix. `corepack enable`
      // answers "not installed"; on Windows pnpm is `pnpm.cmd`, which
      // `execFileSync` will not find without a shell — and `pnpm exec …`
      // cannot be the advice for either, since it needs the pnpm this arm
      // fired for the absence of.
      return fail(
        "could not run `pnpm` — `execFileSync` could not spawn it",
        "install it (`corepack enable`); on Windows it is `pnpm.cmd`, which this call cannot find without a shell"
      );
    }
    return fail("`pnpm ls` failed", cause.message.split("\n")[0]);
  }

  let projects;
  try {
    projects = JSON.parse(out);
  } catch (cause) {
    return fail("`pnpm ls` did not return JSON", `${cause.message} — got: ${out.slice(0, 200)}`);
  }
  if (!Array.isArray(projects)) return fail("`pnpm ls` returned JSON that is not an array");

  // A LIVE CHECK, not a protection-shaped one — measured, because the obvious
  // reading is that it cannot fire. `pnpm run` and `pnpm exec` do exclude the
  // workspace root from a recursive selection unless `--include-workspace-root`
  // is passed; `pnpm ls -r` does NOT, and lists it:
  //
  //     pnpm ls -r --depth -1 --json        → includes `oxy -> <repo root>`
  //
  // So a dropped or broken `--filter` puts the root in this list, and the gate
  // would go on to demand `typecheck`/`test`/`build` of the repo root and fail
  // for a reason that names the wrong thing. This catches that first.
  if (projects.some((p) => resolve(p.path) === ROOT)) {
    return fail(
      `\`${FILTER}\` selected the workspace root`,
      "the filter is not narrowing — refusing to check a set nobody asked for"
    );
  }
  return projects;
}

const projects = selected();
if (projects.length === 0) {
  // Bare `fail(…)`, not `return fail(…)`: this is module top level, where a
  // `return` is a syntax error. Inside a function the `return` is what makes
  // the terminality local rather than a fact about `process.exit`.
  fail(
    `\`--filter "${FILTER}"\` selected no projects`,
    FILTER === DEFAULT_FILTER
      ? "has `sdk/` moved, or did its members leave `pnpm-workspace.yaml`?"
      : "SDK_FILTER is narrowed — is that deliberate?"
  );
}

const missing = [];
for (const project of projects) {
  const pkg = JSON.parse(readFileSync(join(project.path, "package.json"), "utf8"));
  // `relative`, not `slice(ROOT.length + 1)`: the two paths agree today (both
  // derive from ROOT), but where they ever did not — a symlinked checkout, the
  // physical-vs-logical class fixed in `oxyc validate` — slice emits a
  // truncation of an unrelated path into a failure message instead of failing.
  const rel = relative(ROOT, project.path);
  for (const script of REQUIRED) {
    if (!pkg.scripts?.[script]) missing.push(`${rel} declares no \`${script}\` script`);
  }
}

if (missing.length > 0) {
  fail(
    "These packages are selected by the `sdk` job's filter and would be skipped:\n\n" +
      missing.map((line) => `  ${line}`).join("\n"),
    "Add the script, or drop the package from the workspace deliberately — a package\n  " +
      "selected and skipped leaves a blocking step green having checked nothing."
  );
}

console.log(
  `ok — all ${projects.length} package(s) matching \`${FILTER}\` declare ${REQUIRED.join(", ")}`
);
