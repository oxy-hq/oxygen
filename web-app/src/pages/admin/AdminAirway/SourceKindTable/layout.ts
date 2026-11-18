/**
 * The source-kind table's columns, in one place so the header and every row agree.
 *
 * Two layouts, chosen by the table's own width (a container query, not the viewport:
 * the admin rail takes a different share of every window):
 *
 * - **from `@3xl` (48rem)**: kind · policy · environment · overrides · updated · save.
 *   The minimums add up to about 45rem with the gaps and padding, so it never has to
 *   overflow at the width it switches on.
 * - **below it**: Updated is dropped and policy and environment shrink to nothing
 *   (their cells truncate), so the row keeps its kind, its values and, above all, its
 *   Save. The line's Save is the only one there is: the editor has none of its own.
 *
 * The last column is fixed in both, so a row's "Unsaved · Save" appearing does not
 * shift every other column. The container still scrolls sideways rather than clipping,
 * as a backstop for a width narrower than either layout.
 */
export const SOURCE_KIND_COLUMNS =
  "grid-cols-[minmax(6rem,auto)_minmax(0,1fr)_minmax(0,1fr)_auto_7.5rem] @3xl:grid-cols-[minmax(7rem,auto)_minmax(8rem,1fr)_minmax(7rem,1fr)_auto_minmax(4rem,auto)_7.5rem]";

/** A cell that exists only in the wide layout. Header and rows must hide the same cells. */
export const WIDE_ONLY = "hidden @3xl:flex";
