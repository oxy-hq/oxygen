/**
 * Does `selected` include every one of `held`?
 *
 * A set test, never `selected.length === held.length`: `held` is live query
 * data and can change under a standing selection, and equal counts over
 * different members would read a partial selection as "every resource".
 */
export const coversEvery = (held: readonly string[], selected: readonly string[]): boolean =>
  held.length > 0 && held.every((r) => selected.includes(r));
