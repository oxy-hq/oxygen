/**
 * The slice of `@oxy-hq/sdk/theme-guard` that the `design_system` promote
 * check uses, declared here.
 *
 * For the reason `sdk-testing.d.ts` gives: this package compiles with
 * `noUncheckedIndexedAccess` and the SDK does not, so mapping the module's
 * SOURCE in `tsconfig.json` `paths` would make `tsc` report errors in files
 * this package does not own. `tsdown.config.ts` and `vitest.config.ts` alias
 * the module to that source, so the bundle carries the real guard and a
 * published `oxyc` depends on no SDK package at run time; only the types are
 * restated. `checks/design-system-guard.test.ts` runs the real guard on built
 * CSS, so a drifted shape fails there.
 */
declare module "@oxy-hq/sdk/theme-guard" {
  /** One place a stylesheet works against the SDK theme. */
  export interface ThemeFinding {
    /** The file as it was given. */
    file: string;
    /** 1-based line in that file. */
    line: number;
    /** `file:line`. */
    at: string;
    /** `restates-sdk-token`, `bare-mode-class`, … */
    rule: string;
    /** The custom property, selector or at-rule the finding is about. */
    subject: string;
    message: string;
  }

  /** CSS (or HTML) held in memory, named for the findings. */
  export interface ThemeCssSource {
    file: string;
    css: string;
  }

  export interface CheckThemeCssOptions {
    /** Read the sources as an app's built CSS instead of its source. */
    built?: boolean;
  }

  export function checkThemeCss(
    files: Iterable<string | ThemeCssSource>,
    options?: CheckThemeCssOptions
  ): ThemeFinding[];

  /** Every custom property the SDK theme defines (`--card`): the CSS digest leaves them out. */
  export const SDK_THEME_NAMES: ReadonlySet<string>;
}
