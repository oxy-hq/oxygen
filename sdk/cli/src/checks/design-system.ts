/**
 * The `design_system` promote check: the build's `@oxy-hq/sdk` at or above the
 * platform's floor, and the SDK's theme guard on the build's CSS (every
 * stylesheet under `assets/`, and the `<style>` blocks of `index.html`).
 *
 * The guard is the SDK's own `checkThemeCss`, read in built-CSS mode: the
 * same rules and the same words an app's own test prints, on what Vite and
 * Tailwind compiled them into.
 *
 * VERDICT, in this order. The guard always runs, and any finding fails the
 * build whatever its SDK version. An SDK below the floor fails it. A build
 * that does not say which SDK it was built with (published by an older
 * `oxyc`, or straight through the API) is `skipped`, "SDK version unknown",
 * when the guard is clean. Otherwise it passed.
 */

import { checkThemeCss, type ThemeFinding } from "@oxy-hq/sdk/theme-guard";
import { type Finding, finding, type Outcome, outcome } from "./outcome.js";

/** One of the build's files, as the platform read it from the build store. */
export interface StyleSource {
  file: string;
  css: string;
}

type Version = [number, number, number];

/** The first `major.minor.patch` in `raw` (`2.30.1`, `^2.30.0`, `2.31.0-rc.1`). */
export function parseVersion(raw: string): Version | undefined {
  const start = raw.search(/[0-9]/);
  if (start < 0) return undefined;
  const parts = raw
    .slice(start)
    .split(/[^0-9]/)
    .filter((part) => part !== "")
    .slice(0, 3)
    .map(Number);
  const [major, minor, patch] = parts;
  if (major === undefined || minor === undefined || patch === undefined) return undefined;
  return parts.every(Number.isSafeInteger) ? [major, minor, patch] : undefined;
}

/** Whether `raw` is at or above `floor`; `undefined` when it is no version. */
export function meetsFloor(raw: string, floor: Version): boolean | undefined {
  const version = parseVersion(raw);
  if (!version) return undefined;
  for (let i = 0; i < 3; i += 1) {
    const [have, need] = [version[i] as number, floor[i] as number];
    if (have !== need) return have > need;
  }
  return true;
}

/** A finding in a few words: `restates --card`. */
function short(found: ThemeFinding): string {
  return found.rule === "restates-sdk-token"
    ? `restates ${found.subject}`
    : `${found.rule}: ${found.subject}`;
}

/** What is wrong with the SDK version, if anything: the words, and whether it is a verdict. */
function sdkProblem(
  sdk: string | null,
  meets: boolean | undefined,
  floor: string
): string | undefined {
  if (sdk === null) {
    return `SDK version unknown: the build does not say which @oxy-hq/sdk it was built with (an oxyc that sends it records it; the floor is ${floor})`;
  }
  if (meets === true) return undefined;
  return meets === false
    ? `@oxy-hq/sdk ${sdk} is below the floor ${floor}`
    : `@oxy-hq/sdk "${sdk}" is not a version`;
}

/** The verdict over the SDK version and the theme findings. */
export function judgeDesignSystem(
  sdk: string | null,
  floor: string,
  theme: readonly ThemeFinding[],
  files: readonly string[]
): Outcome {
  const extra = { sdk_version: sdk, sdk_floor: floor, files };
  const floorVersion = parseVersion(floor);
  const findings: Finding[] = theme.map((found) =>
    finding("error", found.message, { at: found.at, rule: found.rule })
  );
  if (!floorVersion) {
    const problem = `the platform's SDK floor "${floor}" is not a version`;
    findings.push(finding("error", problem, { rule: "sdk-floor" }));
    return outcome(theme.length > 0 ? "failed" : "error", problem, findings, extra);
  }
  const meets = sdk === null ? undefined : meetsFloor(sdk, floorVersion);
  const problem = sdkProblem(sdk, meets, floor);
  if (problem !== undefined) {
    // Unknown is no verdict on the SDK, so it is not an error finding.
    findings.push(finding(sdk === null ? "info" : "error", problem, { rule: "sdk-floor" }));
  }
  const first = theme[0];
  if (first !== undefined) {
    const more = theme.length > 1 ? ` (+${theme.length - 1} more)` : "";
    return outcome("failed", `${first.at} ${short(first)}${more}`, findings, extra);
  }
  if (problem === undefined) return outcome("passed", `SDK ${sdk} · guard clean`, findings, extra);
  if (meets === false) return outcome("failed", problem, findings, extra);
  // No recorded version is nothing to judge; a recorded one that is no version is broken.
  return outcome(sdk === null ? "skipped" : "error", problem, findings, extra);
}

/** Run the guard on the build's CSS and judge it with the SDK version. */
export function checkDesignSystem(
  sources: readonly StyleSource[],
  sdk: string | null,
  floor: string
): Outcome {
  const theme = checkThemeCss(sources, { built: true });
  return judgeDesignSystem(
    sdk,
    floor,
    theme,
    sources.map((source) => source.file)
  );
}
