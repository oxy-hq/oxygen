// `@oxy-hq/sdk/theme-guard` — the theme guard, alone.
//
// `checkThemeCss` finds every place a stylesheet works against the SDK theme (a restated
// token, a literal colour in `@theme`, a bare `.dark` / `.light`, an own `color-scheme` or mode
// block). `@oxy-hq/sdk/testing` exports it for an app's own tests, on its source CSS; this entry
// is for a Node tool that needs the guard and nothing else: `oxyc` runs it with
// `{ built: true }` on an app's built CSS for the platform's design-system check.
//
// Node only (`node:fs`, `node:path`). No React, no DOM, no test runner, and none of the rest of
// `@oxy-hq/sdk/testing`: `src/testing/themeGuardBuilt.test.ts` holds its imports to that.

export {
  type CheckThemeCssOptions,
  checkThemeCss,
  formatThemeFindings,
  SDK_THEME_NAMES,
  type ThemeCssSource,
  type ThemeFinding,
  type ThemeGuardRule
} from "./testing/themeGuard";
