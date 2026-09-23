import { AlertTriangle } from "lucide-react";
import type React from "react";

import { Button } from "@/components/ui/shadcn/button";

/**
 * What the server said when it declined a cursor rewind, **in full**.
 *
 * The backend writes these sentences carefully: each names the table, says why
 * re-pulling it would not converge, and — for a reset it could not scope — gives
 * both readings of an unclaimed table rather than one. They are the entire
 * factual basis for the decision an operator is about to make, so this component
 * renders each one exactly as it arrived: no truncation, no summarising, no
 * counting them up into "2 problems found".
 *
 * Why that matters concretely: the destructive sibling of this action drops
 * every destination table. An operator who reaches for `force` after reading a
 * generic "Reset failed" has overridden something they were never shown.
 *
 * The heading deliberately does **not** repeat the wire's `error:
 * "would_duplicate"` label. That field is hardcoded and also covers the
 * could-not-scope refusal, which is not a duplication claim — so stating it
 * here would put a wrong word above right ones. This component branches on
 * nothing in the prose; it just prints it.
 */
const RefusalPanel: React.FC<{
  /** The server's reasons, already rendered. Rendered verbatim, one per line. */
  reasons: string[];
  /** What the refused attempt was scoped to, for the override restatement. */
  scopeLabel: string;
  /** Whether the override has been revealed. Owned by the parent so that
   *  changing the selection can retract it. */
  overrideRevealed: boolean;
  onRevealOverride: () => void;
  onForce: () => void;
  forcing: boolean;
}> = ({ reasons, scopeLabel, overrideRevealed, onRevealOverride, onForce, forcing }) => (
  <div
    data-testid='airway-cursor-refusal'
    className='space-y-3 rounded-md border border-destructive/40 bg-destructive/5 p-3'
  >
    <div className='flex items-start gap-2'>
      <AlertTriangle className='mt-0.5 h-4 w-4 shrink-0 text-destructive' />
      <div>
        <p className='font-medium text-destructive text-sm'>The server refused this rewind.</p>
        <p className='mt-0.5 text-muted-foreground text-xs'>
          Nothing was changed. Every reason it gave, in full:
        </p>
      </div>
    </div>

    <ul data-testid='airway-cursor-refusal-reasons' className='space-y-2'>
      {reasons.map((reason) => (
        <li
          key={reason}
          className='whitespace-pre-wrap break-words rounded border border-border bg-background p-2 font-mono text-xs leading-relaxed'
        >
          {reason}
        </li>
      ))}
    </ul>

    {/* `force` is not on screen until the reasons above have been rendered, and
        it is a disclosure rather than a checkbox: a checkbox sitting beside the
        button can be ticked by someone who never read them. */}
    {overrideRevealed ? (
      <div
        data-testid='airway-cursor-override'
        className='space-y-2 border-destructive/40 border-t pt-3'
      >
        <p className='text-xs'>
          Overriding accepts{" "}
          <span className='font-medium'>
            all {reasons.length} reason{reasons.length === 1 ? "" : "s"} above
          </span>
          . The next run re-pulls {scopeLabel}, and where a table appends, that lands duplicate rows
          rather than corrected ones.{" "}
          <span className='font-medium'>A later run will not remove them.</span>
        </p>
        <Button
          size='sm'
          onClick={onForce}
          disabled={forcing}
          className='bg-destructive text-destructive-foreground hover:bg-destructive/90'
        >
          {forcing ? "Rewinding…" : "Rewind anyway"}
        </Button>
      </div>
    ) : (
      <Button
        size='sm'
        variant='ghost'
        onClick={onRevealOverride}
        data-testid='airway-cursor-reveal-override'
        className='h-auto p-0 text-muted-foreground text-xs underline hover:bg-transparent hover:text-destructive'
      >
        Override this refusal
      </Button>
    )}
  </div>
);

export default RefusalPanel;
