/**
 * The query keys of a build's checks.
 *
 * They are their own module, not entries of `customApps` in `../queryKey.ts`: that
 * file is past the size the code-metrics gate allows, so it takes no new keys. The
 * values still sit under the `customApps` prefix where the comments below say so,
 * which is what keeps `queryKeys.customApps.all()` invalidating them.
 */
export const buildChecksKeys = {
  /** One build's latest checks; `build` is its row id. Under `appBuildChecks(id)`, so a
   *  rerun refetches every build's, and under the `customApps` prefix, so a production
   *  move does too. */
  buildChecks: (id: string, build: string) => ["customApps", "buildChecks", id, build] as const,
  /** The attempt run on production for one build, under that build's `buildChecks`. */
  buildChecksOnProduction: (id: string, build: string) =>
    ["customApps", "buildChecks", id, build, "production"] as const,
  appBuildChecks: (id: string) => ["customApps", "buildChecks", id] as const,
  /** One file of one attempt (a screen's picture). Outside the `customApps` prefix on
   *  purpose: a file of an attempt does not change, so a rerun, a re-read of the checks
   *  or a production move must not fetch every picture on screen again. */
  buildCheckFile: (id: string, build: string, environment: string, attempt: number, name: string) =>
    ["customAppBuildCheckFiles", id, build, environment, attempt, name] as const
};
