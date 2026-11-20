// Types for the desk UI's CSS build, for the tests that import it (src/ui/uiCss.test.ts). The
// script is plain JavaScript so tsdown's onSuccess needs no build.

/** The groups, in the order their CSS follows base.css. */
export const GROUPS: readonly string[];
/** The layer order ui.css and tailwind.css both state. */
export const LAYER_ORDER: string;
/** The source files, base first, relative to src/ui. */
export function uiCssSources(): string[];
/** dist/ui.css as a string, from the package at `root` (default: this package). */
export function buildUiCss(root?: string): string;
