/**
 * The reference design language's class recipes, as Tailwind classes: the
 * SDK's `ui` recipes (`sdk/typescript/src/ui/recipes.ts` and `base.css`), which
 * the reference app's screens are written in. Names match the SDK's, so a
 * screen here can be checked against the reference screen it copies, name for
 * name.
 *
 * They look right only inside a `design-reference` subtree
 * (`styles/reference/theme.css`), which supplies the palette and the square
 * corners. The shell and the home carry that class.
 */

/** Put this on the root of a subtree drawn in the reference design language. */
export const DESIGN_REFERENCE = "design-reference";

// ─── Type ───────────────────────────────────────────────────────────────
/** 36px semibold: a page's display line, the home's greeting. */
export const text2xl = "text-ref-2xl font-semibold tabular-nums";
/** 16px medium: an item's lead line, an app card's name. */
export const textBase = "text-ref-base font-medium";
/** 14px: controls and rows. */
export const textMd = "text-ref-md";
/** 13px: body text and notes. */
export const textSm = "text-ref-sm";
/** 12px semibold capitals: a label, never prose. */
export const textXs = "text-ref-xs font-semibold uppercase tracking-eyebrow";

/** A section's small capitalised label. */
export const eyebrow = `${textXs} text-subtle-foreground`;
/** A note under a lead line: freshness, a row's detail. */
export const note = `${textSm} text-muted-foreground`;

// ─── Surfaces ───────────────────────────────────────────────────────────
/** A panel or card on the canvas. */
export const card = "border border-border-subtle bg-card";
/** A boxed item inside or beside a panel: the field surface on a hairline. */
export const tile = "border border-border-subtle bg-field";

// ─── Focus ──────────────────────────────────────────────────────────────
/**
 * The one focus ring: 1.5px of the muted ink, from the keyboard only. The
 * outline is made transparent rather than removed, so Windows High Contrast
 * still draws one.
 */
export const focusRing =
  "focus-visible:outline-2 focus-visible:outline-transparent focus-visible:ring-[1.5px] focus-visible:ring-(--focus-ring-color) focus-visible:ring-offset-0";
/** The ring drawn inside the control, for segments that butt together. */
export const focusRingInset = `${focusRing} focus-visible:ring-inset`;

// ─── Segments (which slice of this list) ────────────────────────────────
/** The strip: a field-coloured track, its segments flush, hairlines between. */
export const segmentRow =
  "inline-flex w-fit items-stretch divide-x divide-border-subtle overflow-hidden border border-border-subtle bg-segment-track";
/** One segment: 36px, the control padding. */
export const segment = `inline-flex min-h-9 items-center gap-2 px-3 ${textMd} ${focusRingInset}`;
/** Selected: the fill alone, no outline. */
export const segmentOn = "bg-segment-selected font-semibold text-foreground";
export const segmentOff =
  "font-medium text-dim-foreground hover:bg-row-hover hover:text-foreground";

// ─── Buttons ────────────────────────────────────────────────────────────
const btn =
  "inline-flex shrink-0 items-center justify-center border border-transparent font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50";
/** The compact control: 32px, 14px text. Add a fill below. */
export const btnCompact = `${btn} min-h-8 gap-1.5 px-3 ${textMd} ${focusRing}`;
/** The action a surface exists to offer, one per surface, in the brand fill. */
export const btnCompactPrimary = "bg-brand text-inverse-foreground enabled:hover:bg-brand-hover";
/** Everything else beside it: the button surface on a hairline. */
export const btnCompactSecondary =
  "border-border-subtle bg-btn-surface text-foreground enabled:hover:bg-btn-surface-hover";
/** A text action: no fill, no border, no side padding; dim ink going to full. */
export const btnText = `${btn} min-h-8 w-fit justify-start gap-1.5 border-0 px-0 ${textMd} text-dim-foreground enabled:hover:text-foreground ${focusRing}`;

/** The compact select: a native select on the field surface, 32px. */
export const select = `select-chevron h-8 border border-field-border bg-field pr-8 pl-3 ${textMd} text-foreground ${focusRing}`;

// ─── Rows ───────────────────────────────────────────────────────────────
/** A row in a navigation column: selected is a neutral fill and full ink, no border. */
export const sectionLinkOn = "bg-row-selected font-medium text-foreground";
export const sectionLinkOff = "text-subtle-foreground hover:bg-row-hover hover:text-foreground";

// ─── Icons (Phosphor, regular weight) ───────────────────────────────────
/** 14px, inside text, a tag or a chip. */
export const iconInline = "size-3.5 shrink-0";
/** 16px, in a control. */
export const iconControl = "size-4 shrink-0";
