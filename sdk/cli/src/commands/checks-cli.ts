/**
 * Wires `oxyc checks …` onto the program: an app's own checks, a build's
 * promote checks, and what those found on a build.
 *
 * Registered by `main.ts`, at the place the group has in `oxyc --help`. The
 * verbs are tested on their own: `./checks.ts` (an app's checks),
 * `./checks-build.ts` (`run --build`) and `./checks-show.ts` (`show`).
 */

import type { Command } from "commander";
import { createContext } from "../context/resolve.js";
import { runChecks } from "./checks.js";
import { runBuildChecks } from "./checks-build.js";
import { runChecksShow } from "./checks-show.js";
import type { Wiring } from "./events-cli.js";

type Opts = Record<string, unknown>;

export function registerChecks(program: Command, w: Wiring): void {
  const checks = program
    .command("checks")
    .description(
      "run an app's checks (functions marked \"check\": true) or a build's promote checks, or show what those found on a build"
    );
  registerRun(checks, w);
  registerShow(checks, w);
}

/** Any of these means the promote-checks mode; it refuses a partial set by name. */
function asksForBuildChecks(opts: Opts): boolean {
  return (
    opts.build !== undefined ||
    opts.attempt !== undefined ||
    Boolean(opts.report) ||
    opts.environment !== undefined
  );
}

function registerRun(checks: Command, w: Wiring): void {
  w.withGlobals(
    checks
      .command("run <app>")
      .description(
        "run every check of <org>/<app> (or an app id) and wait for results; with --build, run a build's promote checks"
      )
      .option("--json", "emit results as JSON")
      .option("--timeout <seconds>", "per-check timeout", "300")
      .option("--app-env <environment>", "run against this environment instead of production")
      .option(
        "--browser",
        "also load the app in headless Chromium, in --app-env, and prove it mounted (needs Playwright)"
      )
      .option(
        "--expect-selector <css>",
        "with --browser: a selector that must be visible once the app is ready"
      )
      .option(
        "--build <build>",
        "run this build's promote checks instead (its row id or build_id); needs --report"
      )
      .option("--attempt <n>", "with --build: which attempt of its checks (default: the newest)")
      .option("--report", "with --build: write each check's verdict to the platform (required)")
      .option(
        "--environment <environment>",
        "with --build: staging (the default), or production: the check a promote queues"
      )
      .addHelpText(
        "after",
        "\nWith --build this runs a build's promote checks, the ones `oxyc checks show` prints: it\n" +
          "claims the attempt, waits for the platform's verdict on the build's check functions,\n" +
          "loads staging in headless Chromium at desk (1440x900) and phone (390x844) width, runs\n" +
          "the SDK theme guard on the build's CSS, and, where the app's org holds promotes on its\n" +
          "checks, opens every screen at both widths and pictures each on staging and production\n" +
          "to say what changed. Each check is reported as it finishes. --environment production\n" +
          "runs the one check that follows a promote: it waits until production serves the build,\n" +
          "opens every screen, and reports what the move broke. The page is opened as a reader:\n" +
          "no write is sent and no function is called, on staging or on production.\n" +
          "It is what the platform's GitHub Actions workflow runs; --report is required\n" +
          "because it writes. An app id makes no lookup; <org>/<app> needs a person's login.\n" +
          "--timeout, --app-env, --browser and --expect-selector belong to the other mode.\n" +
          "\nExit 0 when every check has a verdict and none failed, or the attempt was already\n" +
          "closed; 9 (CHECK_FAILED) when a verdict is failed or error; 4, 5, 6 or 7 when a\n" +
          "request was refused or a verdict could not be written.\n"
      )
  ).action(async (app: string, opts: Opts) => {
    const ctx = createContext(w.globals(opts));
    if (asksForBuildChecks(opts)) {
      await runBuildChecks(ctx, app, {
        build: opts.build as string | undefined,
        attempt: opts.attempt as string | undefined,
        report: Boolean(opts.report),
        environment: opts.environment as string | undefined,
        json: Boolean(opts.json),
        appEnv: opts.appEnv as string | undefined,
        browser: Boolean(opts.browser),
        expectSelector: opts.expectSelector as string | undefined
      });
      return;
    }
    await runChecks(ctx, app, {
      json: Boolean(opts.json),
      timeoutSeconds: Number(opts.timeout),
      appEnv: opts.appEnv as string | undefined,
      browser: Boolean(opts.browser),
      expectSelector: opts.expectSelector as string | undefined
    });
  });
}

function registerShow(checks: Command, w: Wiring): void {
  w.withGlobals(
    checks
      .command("show <app>")
      .description(
        "what the promote checks found on a build of <org>/<app> (or an app id): staging's, unless --build"
      )
      .option("--build <id>", "a build's build_id or row id, instead of the one staging serves")
      .option("--attempt <n>", "an earlier attempt of the checks (default: the latest)")
      .option(
        "--environment <environment>",
        "staging (the default), or production: the check that followed a promote"
      )
      .option("--wait", "poll until no check is queued or running, then exit by the result")
      .option("--timeout <seconds>", "with --wait: how long to wait", "600")
      .option("--json", "print the server's answer as one JSON object")
      .addHelpText(
        "after",
        "\nEvery build that reaches staging is checked, by a runner the platform starts: it builds\n" +
          'and validates, its "check": true functions pass, it loads on desk and phone, it follows\n' +
          "the design system; where the app's org holds promotes on its checks, every screen opens\n" +
          "too, and What changed says how many screens differ from production and whether a person\n" +
          "has seen them. This prints who ran the checks, one line per check, the findings under one\n" +
          "that failed, a model's notes under a check (marked as a model's), then the result.\n" +
          "--environment production reads the check that ran after a promote, on the build\n" +
          "production serves. Whether a promote waits on a check is the plan's to say: `oxyc promote\n" +
          "<app> --dry-run`. An attempt no runner answered says so: run it again from the Builds page.\n" +
          "\nExit 0 when they passed, are still running or none ran; 9 (CHECK_FAILED) when\n" +
          "they failed; with --wait, 7 when --timeout passed with checks still running.\n" +
          "\nThe checks are a staging build's evidence, so the server answers only a caller who\n" +
          "may open the app's staging (Oxy staff with develop_apps over its org): a person's\n" +
          "login or a token carrying that standing. A publish token is answered not-found (5),\n" +
          "as for a staging run it reads back; a sandbox agent token is refused before any request.\n"
      )
  ).action(async (app: string, opts: Opts) => {
    await runChecksShow(createContext(w.globals(opts)), app, {
      build: opts.build as string | undefined,
      attempt: opts.attempt as string | undefined,
      environment: opts.environment as string | undefined,
      wait: Boolean(opts.wait),
      timeoutSeconds: Number(opts.timeout),
      json: Boolean(opts.json)
    });
  });
}
