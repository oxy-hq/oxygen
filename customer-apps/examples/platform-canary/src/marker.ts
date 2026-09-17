// marker — the attributes on the page's `<main>` that the release checks' browser
// check reads, decided from page state.
//
// Kept out of App.tsx so the one decision the release journey's browser check
// trusts is tested on its own (marker.test.ts):
//
//   every check passed    data-canary="ready"
//   a check failed        data-canary-failed="<check-name>" (the first, in check order)
//   manifest didn't load  data-canary-failed="manifest"
//   checks still running  neither attribute

import type { CheckName } from "./checks";

export type Outcome = "pass" | "fail";

/** The order the page runs its checks in. */
export const CHECK_ORDER: CheckName[] = ["echo", "sql_query_route", "sdk_query"];

export type PageState =
  | { manifest: "failed" }
  | { manifest: "loaded"; outcomes: Partial<Record<CheckName, Outcome>> };

export interface CanaryMarker {
  "data-canary"?: "ready";
  "data-canary-failed"?: CheckName | "manifest";
  "aria-busy"?: boolean;
}

export function canaryMarker(state: PageState): CanaryMarker {
  if (state.manifest === "failed") return { "data-canary-failed": "manifest" };
  const { outcomes } = state;
  const failed = CHECK_ORDER.find((check) => outcomes[check] === "fail");
  if (failed) return { "data-canary-failed": failed, "aria-busy": false };
  if (CHECK_ORDER.every((check) => outcomes[check] === "pass")) {
    return { "data-canary": "ready", "aria-busy": false };
  }
  return { "aria-busy": true };
}
