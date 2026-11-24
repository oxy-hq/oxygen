// The `data-*` props a piece puts on its root element (an app's `data-testid`, a guided tour's
// `data-tour`): `ParentTabs` (and `TopBarTabs` under it) since 2.35.0, `Panel` likewise. Only
// `data-*` keys pass; every other unknown prop is still dropped, so a piece's own attributes and
// handlers stay its own.

/** The props type a piece adds to take `data-*` attributes for its root. */
export type DataAttributes = { [data: `data-${string}`]: string | number | boolean | undefined };

/** The `data-*` entries of a props object, for a piece's root element. */
export function dataAttributes(props: object): Record<`data-${string}`, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(props)) if (k.startsWith("data-")) out[k] = v;
  return out;
}
