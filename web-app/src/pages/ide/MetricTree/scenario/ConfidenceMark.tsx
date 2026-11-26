import { cn } from "@/libs/shadcn/utils";
import type { ImpactConfidence } from "@/types/metricTree";

interface Mark {
  /** Carried on the canvas, where a word does not fit at 200px. */
  glyph: string;
  /** Carried in the side panel, where it does — and where the glyph gets
   *  learned, since the two always render together there. */
  label: string;
  help: string;
  className: string;
}

/**
 * Colour carries the distinction, border style reinforces it: solid `success`
 * for arithmetic, dashed `warning` for an approximation. Two reasons for those
 * two tokens rather than a pair of greys —
 *
 * 1. A one-character glyph in `muted-foreground` on `muted` is invisible at
 *    this size, and it sits beside coloured figures that win the eye outright.
 * 2. `success` vs `warning` is already this codebase's verified-vs-not axis
 *    (see the artifact Verified badge), and `success` is the colour the canvas
 *    strokes component edges with — the very edges that make a number exact.
 *
 * Deliberately NOT `info`/`destructive`: those two are spoken for in the same
 * row, where they mean the move went up or down.
 */
const MARKS: Record<ImpactConfidence, Mark> = {
  exact: {
    glyph: "=",
    label: "exact",
    help: "Exact: every hop into this measure is an additive component identity (Δparent = sign × Δchild), so this is arithmetic rather than a forecast.",
    className: "border-success/50 bg-success/10 text-success"
  },
  estimated: {
    glyph: "≈",
    label: "estimated",
    help: "Estimated: the change crossed a driver coefficient, or a × / ÷ component edge linearized at the current value — a first-order approximation, exact only for small moves.",
    className: "border-warning/60 border-dashed bg-warning/10 text-warning"
  },
  unquantifiable: {
    glyph: "?",
    label: "can't size",
    help: "This path crosses a multiplicative edge the model can't size from the values available. The impact is real; its magnitude is unknown, not zero.",
    className: "border-border bg-muted text-muted-foreground"
  }
};

/** The same sentence the glyph's tooltip carries, for the surfaces with room to
 *  print it outright. Exported rather than duplicated: an expanded impact
 *  explains its own confidence in prose, and two copies of this wording would
 *  drift the moment the engine's rules did. */
export function confidenceHelp(confidence: ImpactConfidence): string {
  return MARKS[confidence].help;
}

/**
 * How much of a claim a scenario number is making, marked on every surface
 * that shows one.
 *
 * Without it an exact component identity and a first-order approximation
 * through a fitted coefficient render in the same type, and the surface
 * overstates half of what it shows — `confidence` came back from `predict` for
 * exactly this reason and was being dropped.
 */
export function ConfidenceMark({
  confidence,
  withLabel = false
}: {
  confidence: ImpactConfidence;
  withLabel?: boolean;
}) {
  // `ImpactConfidence` narrows an untyped wire string, so a value the server
  // grows later reaches here as a key `MARKS` has no entry for — and an
  // unguarded `mark.className` throws during render, blanking the whole
  // canvas. Degrading to "can't size" understates one impact; the alternative
  // loses the page.
  const mark = MARKS[confidence] ?? MARKS.unquantifiable;
  return (
    <span
      className={cn(
        // A glyph alone has to carry the whole meaning, so it gets the size a
        // word would not need: 10px semibold, not the 9px of the LEVER chip.
        "shrink-0 border px-1 py-px font-mono font-semibold text-[10px] leading-none",
        mark.className
      )}
      title={mark.help}
      data-testid={`scenario-confidence-${confidence}`}
    >
      {withLabel ? `${mark.glyph} ${mark.label}` : mark.glyph}
    </span>
  );
}
