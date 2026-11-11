// The two name rules the server enforces on `oxy-app.json`, stated once.
//
// The server's own slug and function-name validation is the authority; these
// are their mirrors, and `scripts/gen-manifest-schema.mjs` writes them into
// the JSON Schema (`slug.pattern`, `functions.propertyNames.pattern`) so a
// manifest editor and every validator over the schema enforce the same thing.
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
