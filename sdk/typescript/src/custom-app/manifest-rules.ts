// The two name rules the server enforces on `oxy-app.json`, stated once, and
// the rule for a screen path (`checks.screens`).
//
// The server's own slug and function-name validation is the authority; these
// are their mirrors, and `scripts/gen-manifest-schema.mjs` writes them into
// the JSON Schema (`slug.pattern`, `functions.propertyNames.pattern`,
// `checks.screens.items`) so a manifest editor and every validator over the
// schema enforce the same thing.
//
// No imports and nothing past ES2020: compiled under every package's tsconfig.

/**
 * 1–63 lowercase letters, digits and SINGLE hyphens: no leading, trailing or
 * double hyphen, no underscore. The regex forbids the hyphen shapes
 * structurally; the length is `SLUG_MAX_LEN`, checked separately.
 *
 * A slug becomes the app's served path, a repo path segment and — hyphens to
 * underscores — its OLTP schema and role name, which is why `my_app` is
 * refused rather than normalised: it would alias `my-app` onto one schema.
 */
export const SLUG_PATTERN = /^[a-z0-9]+(-[a-z0-9]+)*$/;

/** Longest slug the server accepts. */
export const SLUG_MAX_LEN = 63;

/** The slug rule as a sentence, for an error that has to say what to change. */
export const SLUG_RULE = `1–${SLUG_MAX_LEN} lowercase letters, digits and single hyphens (no leading, trailing or double hyphen, no underscore)`;

/** The server's `is_valid_slug`. */
export function isValidSlug(slug: string): boolean {
  return slug.length <= SLUG_MAX_LEN && SLUG_PATTERN.test(slug);
}

/**
 * A lowercase letter, then up to 63 lowercase letters, digits or hyphens: the
 * server's `is_valid_function_name`. A manifest key becomes
 * `functions/<name>.js`, so `../../x` has to be refused before any bundler is
 * told to write there.
 */
export const FUNCTION_NAME_PATTERN = /^[a-z][a-z0-9-]{0,63}$/;

/** The function-name rule as a sentence. */
export const FUNCTION_NAME_RULE = `a lowercase letter, then up to 63 lowercase letters, digits or hyphens (${FUNCTION_NAME_PATTERN.source})`;

/** The server's `is_valid_function_name`. */
export function isValidFunctionName(name: string): boolean {
  return FUNCTION_NAME_PATTERN.test(name);
}

/** At most this many (app, function) pairs in a manifest's top-level `calls`. */
export const CALLS_MAX_PAIRS = 64;

/**
 * A screen of the app, as `checks.screens` names one: a path relative to the
 * app's root, `""` for the root itself, with an optional query for an app
 * that picks its screen by one (`"?module=waste"`, `"stores?tab=open"`).
 * Refused structurally: a leading slash (it would name the origin's root, not
 * the app's), a scheme (`https:`, `mailto:`), `..` anywhere in the path, a
 * hash (the checker opens a URL, and a hash never reaches the server or a
 * fresh page load the same way), and a query that is empty or holds a space.
 * The length is `SCREEN_PATH_MAX_LEN`, checked separately.
 */
export const SCREEN_PATH_PATTERN =
  /^(?!\/|[A-Za-z][A-Za-z0-9+.-]*:)(?![^?]*\.\.)[^?#]*(?:\?[^#\s]+)?$/;

/** Longest screen path. */
export const SCREEN_PATH_MAX_LEN = 256;

/** At most this many entries in a manifest's `checks.screens`, each listed once. */
export const SCREENS_MAX = 30;

/** The screen-path rule as a sentence. */
export const SCREEN_PATH_RULE = `a path relative to the app's root ("" is the root itself, "stores/detail" a screen under it, "?module=waste" a screen the app picks by its query), with no leading slash, no hash, no \`..\` in the path, no scheme and no empty or spaced query, at most ${SCREEN_PATH_MAX_LEN} characters`;

/** `true` when `path` may be an entry of `checks.screens`. */
export function isValidScreenPath(path: string): boolean {
  return path.length <= SCREEN_PATH_MAX_LEN && SCREEN_PATH_PATTERN.test(path);
}
