/**
 * What every test of `oxyc checks run --build … --report` on the gate's kinds
 * sets up: a stand-in platform on a loopback port, the checker's credential,
 * the two output streams caught, and one way to run the command and read
 * back what it reported and what files it kept. Test support only: nothing in
 * the shipped bundle imports this.
 *
 * Call {@link useChecksRun} once at the top of a `describe`: it starts a
 * fresh platform before each case and closes it after.
 */

import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, type MockInstance, vi } from "vitest";
import {
  type BuildChecksDeps,
  type BuildChecksOptions,
  type BuildChecksSummary,
  runBuildChecks
} from "../commands/checks-build.js";
import { createContext } from "../context/resolve.js";
import { CliError } from "../util/errors.js";
import {
  APP_ID,
  BUILD_ROW,
  claimOf,
  type FakePlatform,
  startFakePlatform
} from "./fake-checks-platform.js";
import type { FakeChromium } from "./fake-playwright.js";

/** One check as the run reported it (`POST …/checks/report`). */
export interface Reported {
  kind: string;
  status: string;
  summary: string;
  detail: Record<string, unknown> & { findings: { severity: string; message: string }[] };
}

/** How one run of the command ended. */
export interface RunEnd {
  /** The `--json` summary, when one was printed. */
  summary?: BuildChecksSummary;
  /** The error the command ended with: a failed check (exit 9), or a broken run. */
  failure?: CliError;
}

export interface ChecksRunHarness {
  /** The stand-in platform of the case that is running. */
  readonly platform: FakePlatform;
  /** Every wait the run asked for, in order; none is waited. */
  readonly waits: number[];
  /** Make the claim answer the phase-1 claim with `overrides` over it. */
  claim(overrides: Record<string, unknown>): void;
  /** Run the command against `chromium` as the platform's checker. */
  run(
    chromium: FakeChromium,
    deps?: Partial<BuildChecksDeps>,
    options?: Partial<BuildChecksOptions>
  ): Promise<RunEnd>;
  /** What `kind` was reported as; the first check reported when no kind is named. */
  reported(kind?: string): Reported | undefined;
  /** Everything written to a stream, as text. */
  printed(stream: "stdout" | "stderr"): string;
  /** The names of the files the platform kept, sorted. */
  fileNames(): string[];
}

/** A list read out of a reported detail; empty when it is not there. */
export function list<T>(value: unknown): T[] {
  return Array.isArray(value) ? (value as T[]) : [];
}

export function useChecksRun(): ChecksRunHarness {
  let platform: FakePlatform;
  let streams: Record<"stdout" | "stderr", MockInstance>;
  let waits: number[] = [];

  beforeEach(async () => {
    platform = await startFakePlatform();
    waits = [];
    streams = {
      stdout: vi.spyOn(process.stdout, "write").mockImplementation(() => true),
      stderr: vi.spyOn(process.stderr, "write").mockImplementation(() => true)
    };
  });

  afterEach(async () => {
    streams.stdout.mockRestore();
    streams.stderr.mockRestore();
    vi.unstubAllEnvs();
    await platform.close();
  });

  const printed = (stream: "stdout" | "stderr") =>
    // oxlint-disable-next-line typescript/no-base-to-string -- every write here is a string
    streams[stream].mock.calls.map(([chunk]) => String(chunk)).join("");

  return {
    get platform() {
      return platform;
    },
    get waits() {
      return waits;
    },
    claim: (overrides) => {
      platform.on("POST /claim", () => ({
        status: 200,
        body: claimOf(platform.pageUrl, overrides)
      }));
    },
    run: async (chromium, deps = {}, options = {}) => {
      // The job as the workflow runs it: the checker's token in `OXY_API_KEY`, no bearer anywhere.
      vi.stubEnv("OXY_API_KEY", "oxysvc_checker");
      vi.stubEnv("OXY_CHECKS_NO_BEARER", "");
      vi.stubEnv("OXY_CREDENTIALS_PATH", join(tmpdir(), "oxyc-no-such-credentials.json"));
      const ctx = createContext(
        { target: platform.url, tokenEnv: "OXY_CHECKS_NO_BEARER" },
        tmpdir()
      );
      const failure = await runBuildChecks(
        ctx,
        APP_ID,
        { build: BUILD_ROW, attempt: "2", report: true, json: true, ...options },
        {
          loadChromium: async () => chromium,
          pageWaitMs: 40,
          pageSettleMs: 0,
          sleep: async (ms: number) => {
            waits.push(ms);
          },
          env: {},
          ...deps
        }
      ).then(
        () => undefined,
        (cause: unknown) => cause
      );
      if (failure !== undefined && !(failure instanceof CliError)) throw failure;
      const text = printed("stdout");
      return { summary: text ? (JSON.parse(text) as BuildChecksSummary) : undefined, failure };
    },
    reported: (kind) => {
      const bodies = platform.bodies("POST /report") as { checks: Reported[] }[];
      const all = bodies.flatMap((body) => body.checks);
      return kind === undefined ? all[0] : all.find((check) => check.kind === kind);
    },
    printed,
    fileNames: () =>
      platform
        .files()
        .map((file) => file.name)
        .sort()
  };
}
