#!/usr/bin/env node
// Scaffold every create-oxy-app template and prove it installs, builds and
// passes its own tests (the design-system checks among them).
//
// Nothing else in CI touches `sdk/create-oxy-app/templates/**`: they are not
// pnpm workspace packages, so turbo never sees them, and until recently the
// `web-app` path filter did not even match `sdk/**`. That gap is how the
// templates drifted three majors behind what web-app ships, and how two
// install-time breakages reached review instead of a runner — pnpm 10+
// refusing an undeclared dependency build script (`ERR_PNPM_IGNORED_BUILDS`),
// and a `pnpm-workspace.yaml` that detached each app from the customer-apps
// monorepo.
//
// Local `@oxy-hq/*` packages are substituted for the published ranges on
// purpose. The templates are allowed to be ahead of npm — they pin what the
// next publish will produce — so resolving against the registry would make
// this job fail for a release-ordering reason rather than a template defect.
// The range-vs-repo check below covers that axis statically instead.
import { execFileSync, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const REPO = resolve(import.meta.dirname, "../..");
const TEMPLATES_DIR = join(REPO, "sdk/create-oxy-app/templates");
const CLI = join(REPO, "sdk/create-oxy-app/dist/cli.mjs");
// `oxyc`, for the run of a template's own Oxy Function below.
const OXYC = join(REPO, "sdk/cli/dist/main.mjs");

// Templates that ship an Oxy Function, and the function `oxyc fn dev` runs in
// each: the one place a real handler is bundled by the real esbuild and run on
// the SDK's testing context, so what the sample teaches is proven to work.
// `ops` are the host ops the run must record, and `body` what it must answer.
const FUNCTION_RUNS = {
  functions: {
    name: "notify",
    input: '{"name":"Ada"}',
    status: 200,
    ops: ["email.send"],
    body: /"sentTo":"author@example\.test"/
  }
};

// The workspace packages a template may depend on, and where they live.
const LOCAL_PACKAGES = {
  "@oxy-hq/sdk": "sdk/typescript",
  "@oxy-hq/vite-plugin": "sdk/vite-plugin"
};

// Templates that do not build today, each with the reason. They are still
// scaffolded and built, and the failure is still printed — it just does not
// fail the job, so this can land green and start catching NEW rot immediately
// instead of waiting on the existing debt.
//
// The list is self-cleaning: a template that starts passing while still listed
// fails the job, so an entry cannot outlive the bug it documents.
const KNOWN_BROKEN = {};

// The no-copy check every template's own `pnpm test` carries
// (`checkSdkUiCopies()` from `@oxy-hq/sdk/testing`, run by
// `tests/design-system.test.ts`), proved to FAIL in each scaffold, not only to
// pass: a planted local copy of an SDK piece must fail the test with a message
// naming the import to use, the same copy allow-listed with a reason must pass,
// and with the plant removed the test must pass again. A guard that exits 0 on
// a copy is the expensive kind of green.
const PLANT = {
  path: "src/components/SidePanel.tsx",
  copy: "export function SidePanel() {\n  return null;\n}\n",
  allowed:
    "// oxy-ui-local: SidePanel -- a planted variant, to prove the allow-list\n" +
    "export function SidePanel() {\n  return null;\n}\n",
  message: 'import { SidePanel } from "@oxy-hq/sdk/ui"'
};

// `node scripts/ci/verify-templates.mjs vite functions` runs only those, for a
// local loop. CI names none and runs every template.
const ONLY = process.argv.length > 2 ? new Set(process.argv.slice(2)) : null;

const readJson = (p) => JSON.parse(readFileSync(p, "utf8"));

/** Discovered from disk rather than listed here, so a template added tomorrow
 *  is covered without touching this file — the same way the CLI resolves one
 *  (`templates/<id>/`) and the Rust scaffold embeds them (`include_dir!` in
 *  crates/app/src/custom_app_template/registry.rs). */
const templates = () =>
  readdirSync(TEMPLATES_DIR, { withFileTypes: true })
    .filter((e) => e.isDirectory() && existsSync(join(TEMPLATES_DIR, e.name, "package.json")))
    .map((e) => e.name)
    .filter((name) => !ONLY || ONLY.has(name))
    .sort();

function log(msg) {
  process.stdout.write(`${msg}\n`);
}

/** Does `version` satisfy the caret `range`?
 *
 * Hand-rolled because `semver` is not resolvable from this repo root, and
 * wrong the obvious way on the first attempt — worth spelling out. A caret
 * pins the leftmost NON-ZERO component, so it means three different things:
 *
 *     ^1.2.3  ->  >=1.2.3 <2.0.0     (pins major)
 *     ^0.2.0  ->  >=0.2.0 <0.3.0     (pins MINOR — 0.x majors are not compatible)
 *     ^0.0.3  ->  >=0.0.3 <0.0.4     (pins PATCH)
 *
 * That middle case is the one that matters here: `@oxy-hq/vite-plugin` lives
 * in the 0.x range and every template pins it, so treating `^0.2.0` as
 * satisfied by `0.3.0` would print `ok` on exactly the drift this check
 * exists to catch.
 *
 * A prerelease never satisfies a non-prerelease range (`0.3.0-next.1` does
 * not satisfy `^0.3.0`), which changesets can produce.
 *
 * Only caret ranges are used today; anything else is rejected rather than
 * guessed at, so a new range form fails loudly instead of silently passing.
 */
function caretSatisfies(range, version) {
  if (!/^\^\d+\.\d+\.\d+$/.test(range)) return null;
  const [rMaj, rMin, rPat] = range.slice(1).split(".").map(Number);
  if (version.includes("-")) return false;
  const [vMaj, vMin, vPat] = version.split(".").map(Number);
  if (vMaj !== rMaj) return false;
  if (rMaj === 0) {
    // Caret pins the minor, or the patch when the minor is also zero.
    if (rMin === 0) return vMin === 0 && vPat === rPat;
    return vMin === rMin && vPat >= rPat;
  }
  if (vMin !== rMin) return vMin > rMin;
  return vPat >= rPat;
}

// Executable statement of the three caret meanings above. Cheap, and it fails
// at the top of the run rather than by silently passing a template check.
for (const [range, version, want] of [
  ["^1.2.3", "1.2.3", true],
  ["^1.2.3", "1.9.0", true],
  ["^1.2.3", "2.0.0", false],
  ["^1.2.3", "1.2.2", false],
  ["^0.2.0", "0.2.0", true],
  ["^0.2.0", "0.2.9", true],
  ["^0.2.0", "0.3.0", false],
  ["^0.2.0", "1.0.0", false],
  ["^0.0.3", "0.0.3", true],
  ["^0.0.3", "0.0.4", false],
  ["^0.3.0", "0.3.0-next.1", false]
]) {
  if (caretSatisfies(range, version) !== want) {
    throw new Error(`caretSatisfies("${range}", "${version}") should be ${want}`);
  }
}

const failures = [];

/** `oxyc fn dev <name>` in the scaffolded app; throws unless the run answered as `want` says. */
function runFunctionDev(appDir, want) {
  const stdout = execFileSync("node", [OXYC, "fn", "dev", want.name, "--input", want.input], {
    cwd: appDir,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"]
  });
  log(stdout.trimEnd());
  const problems = [];
  if (!stdout.includes(`${want.name} → ${want.status}`)) {
    problems.push(`did not answer ${want.status}`);
  }
  // The whole line: a prefix match would let an extra op after the expected ones through.
  if (!stdout.split("\n").includes(`host ops: ${want.ops.join(", ")}`)) {
    problems.push(`did not record exactly the host ops ${want.ops.join(", ")}`);
  }
  if (!want.body.test(stdout)) problems.push(`its response did not match ${want.body}`);
  if (problems.length) throw new Error(`oxyc fn dev ${want.name} ${problems.join("; ")}`);
}

/** The scaffold's `pnpm test` fails on a planted SidePanel, passes once it is allow-listed, and
 *  passes again once it is removed. */
function provePlantFails(appDir) {
  const plant = join(appDir, PLANT.path);
  mkdirSync(dirname(plant), { recursive: true });
  try {
    writeFileSync(plant, PLANT.copy);
    const run = spawnSync("pnpm", ["test"], { cwd: appDir, encoding: "utf8" });
    const output = `${run.stdout}${run.stderr}`;
    if (run.status === 0)
      throw new Error(`pnpm test passed with a copied SidePanel in ${PLANT.path}`);
    if (!output.includes(PLANT.message)) {
      throw new Error(`pnpm test failed on the plant without naming \`${PLANT.message}\``);
    }
    log(`   plant: a copied SidePanel fails pnpm test, naming ${PLANT.message}`);
    writeFileSync(plant, PLANT.allowed);
    execFileSync("pnpm", ["test"], { cwd: appDir, stdio: ["ignore", "ignore", "inherit"] });
    log("   plant: the same SidePanel, allow-listed with a reason, passes");
    rmSync(plant);
    execFileSync("pnpm", ["test"], { cwd: appDir, stdio: ["ignore", "ignore", "inherit"] });
    log("   plant: removed, pnpm test passes again");
  } finally {
    rmSync(plant, { force: true });
  }
}

// ---------------------------------------------------------------------------
// 1. Static: every @oxy-hq/* range a template pins must be satisfied by the
//    version this repo would publish. Catches templates and packages drifting
//    apart — e.g. templates asking for ^0.2.0 while the plugin is still 0.1.0,
//    which breaks every scaffold's install until someone publishes.
// ---------------------------------------------------------------------------
log("== range check: template @oxy-hq/* pins vs. this repo's versions ==");
/** `pnpm@11.22.0+sha512…` -> `pnpm@11.22.0`. The hash is an integrity suffix,
 *  and `corepack use pnpm@x` — the natural way to bump — writes it on whichever
 *  manifest it touches, so both sides have to be normalised or a routine bump
 *  reports as drift. */
const packageManagerVersion = (v) => (v ? String(v).split("+")[0] : null);
const rootPackageManager = packageManagerVersion(
  readJson(join(REPO, "package.json")).packageManager
);
const localVersions = Object.fromEntries(
  Object.entries(LOCAL_PACKAGES).map(([name, dir]) => [
    name,
    readJson(join(REPO, dir, "package.json")).version
  ])
);
for (const [name, version] of Object.entries(localVersions)) {
  log(`   ${name} @ ${version} (${LOCAL_PACKAGES[name]})`);
}
for (const template of templates()) {
  const pkg = readJson(join(TEMPLATES_DIR, template, "package.json"));
  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  // pnpm 10+ defaults `manage-package-manager-versions` on, so the scaffold's
  // install below SELF-SWITCHES to whatever the template pins — not the pnpm
  // this runner is using. That is the right behaviour (it tests the pnpm a real
  // user gets), but it means a root bump silently leaves this job proving the
  // templates install under the OLD pnpm while every other JS job runs the new
  // one. The failure class this job exists for, `ERR_PNPM_IGNORED_BUILDS`, is
  // itself pnpm-version-gated, so that is exactly the gap where it would stop
  // being evidence.
  const templatePackageManager = packageManagerVersion(pkg.packageManager);
  if (!templatePackageManager) {
    // No pin means no self-switch — the scaffold just uses the runner's pnpm,
    // which IS the one CI uses. Still a failure, for the opposite reason: a
    // published template should pin, or a user's install silently depends on
    // whatever they happen to have.
    failures.push(
      `${template} declares no packageManager — a published template should pin ` +
        `one, or a user's install depends on whichever pnpm they happen to have.`
    );
  } else if (templatePackageManager !== rootPackageManager) {
    failures.push(
      `${template} pins ${templatePackageManager} but the repo root is on ` +
        `${rootPackageManager} — the scaffold's install self-switches to the ` +
        `template's pnpm, so this job stops testing the one CI uses.`
    );
  }

  for (const [name, range] of Object.entries(deps)) {
    if (!(name in localVersions)) {
      // A third @oxy-hq/* package would otherwise skip the range check AND
      // install from the registry in stage 2 — quietly reintroducing the
      // registry coupling this design exists to avoid, with nothing saying so.
      if (name.startsWith("@oxy-hq/")) {
        failures.push(
          `${template} depends on ${name}, which is not in LOCAL_PACKAGES — add it ` +
            `there, or both the range check and the local-resolution substitution ` +
            `silently skip it.`
        );
      }
      continue;
    }
    const ok = caretSatisfies(range, localVersions[name]);
    if (ok === null) {
      failures.push(`${template}: ${name}${range} is not a caret range — extend caretSatisfies().`);
      continue;
    }
    log(`   ${ok ? "ok  " : "FAIL"} ${template}: ${name}${range} vs ${localVersions[name]}`);
    if (!ok) {
      failures.push(
        `${template} pins ${name}${range}, but this repo has ${localVersions[name]}. ` +
          `Bump the package version or relax the template's range — otherwise every ` +
          `scaffold fails at install until a publish catches up.`
      );
    }
  }
}

// ---------------------------------------------------------------------------
// 2. Behavioural: scaffold each template, install it, build it.
// ---------------------------------------------------------------------------
if (!existsSync(CLI)) {
  throw new Error(`create-oxy-app is not built: ${CLI} missing (run its build first)`);
}
// The CI job builds the packages the templates consume but not `oxyc`, which
// only this run's `fn dev` step needs: build it here rather than widen that
// job's filter (and its approval surface) for one consumer.
if (!existsSync(OXYC)) {
  log("== building @oxy-hq/cli (oxyc, for `fn dev`) ==");
  execFileSync("pnpm", ["--filter", "@oxy-hq/cli", "build"], { cwd: REPO, stdio: "inherit" });
}

// Outside the repo on purpose: inside it, pnpm would resolve the scaffold as
// part of this monorepo's workspace and the install would not resemble a real
// user's.
const workRoot = mkdtempSync(join(tmpdir(), "oxy-templates-"));
log(`\n== scaffold + install + build (in ${workRoot}) ==`);

for (const template of templates()) {
  const appName = `probe-${template}`;
  const appDir = join(workRoot, appName);
  log(`\n-- ${template}`);
  // Which stage failed, so KNOWN_BROKEN can excuse only the stage its reason
  // actually describes. Quarantining the whole chain would exempt a listed
  // template from the INSTALL failures this job exists for — a future pnpm
  // major breaking `dashboard`'s install would print "FAILED (known)" under a
  // reason about a missing type export.
  let stage = "scaffold";
  try {
    execFileSync("node", [CLI, appName, "--template", template, "--yes"], {
      cwd: workRoot,
      stdio: "inherit"
    });

    // Point @oxy-hq/* at this checkout so the run tests the templates, not the
    // registry's publish state.
    const pkgPath = join(appDir, "package.json");
    const pkg = readJson(pkgPath);
    for (const field of ["dependencies", "devDependencies"]) {
      for (const name of Object.keys(pkg[field] ?? {})) {
        if (name in LOCAL_PACKAGES) {
          pkg[field][name] = `file:${join(REPO, LOCAL_PACKAGES[name])}`;
        }
      }
    }
    writeFileSync(pkgPath, `${JSON.stringify(pkg, null, 2)}\n`);

    // `--no-frozen-lockfile`: a scaffold ships no lockfile, and the file:
    // substitution above changes the specifiers anyway.
    stage = "install";
    execFileSync("pnpm", ["install", "--no-frozen-lockfile"], {
      cwd: appDir,
      stdio: "inherit"
    });

    for (const script of ["typecheck", "build", "test"]) {
      if (pkg.scripts?.[script]) {
        stage = script;
        execFileSync("pnpm", ["run", script], { cwd: appDir, stdio: "inherit" });
      }
    }
    if (pkg.scripts?.test) {
      stage = "plant";
      provePlantFails(appDir);
    }
    if (template in FUNCTION_RUNS) {
      stage = "fn-dev";
      runFunctionDev(appDir, FUNCTION_RUNS[template]);
    }
    if (template in KNOWN_BROKEN) {
      failures.push(
        `${template} is listed in KNOWN_BROKEN but now builds — delete its entry ` +
          `so the template stays covered.`
      );
      log(`-- ${template}: ok (and no longer known-broken — drop the entry)`);
    } else {
      log(`-- ${template}: ok`);
    }
  } catch (err) {
    const first = err.message.split("\n")[0];
    const excusable = stage === "typecheck" || stage === "build";
    if (template in KNOWN_BROKEN && excusable) {
      log(`-- ${template}: FAILED at ${stage} (known) — ${KNOWN_BROKEN[template]}`);
    } else if (template in KNOWN_BROKEN) {
      failures.push(
        `${template} failed at ${stage}, which KNOWN_BROKEN does not excuse ` +
          `(its reason is a typecheck/build defect): ${first}`
      );
      log(`-- ${template}: FAILED at ${stage} (NOT excused)`);
    } else {
      failures.push(`${template}: failed at ${stage}: ${first}`);
      log(`-- ${template}: FAILED at ${stage}`);
    }
  }
}

rmSync(workRoot, { recursive: true, force: true });

if (failures.length) {
  log(`\n${failures.length} failure(s):`);
  for (const f of failures) log(`  - ${f}`);
  process.exit(1);
}
log("\nall templates scaffold, install, build and pass their tests");
