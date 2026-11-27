// Types for the generator, for the test that imports it (src/testing/themeGuardBuilt.test.ts).
// The script is plain JavaScript so `pnpm gen:theme-guard-rules` needs no build.

/** Where the rules are checked in. */
export const SDK_RULES_PATH: string;
/** The stylesheets an app's bundle can carry, relative to the package. */
export function sdkStylesheets(): string[];
/** Each SDK prelude (as the guard keys it) with the properties the SDK declares under it. */
export function generateSdkRules(root?: string): Record<string, string[]>;
/** The bytes the file holds before Biome wraps it. */
export function renderSdkRules(root?: string): string;
