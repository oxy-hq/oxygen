// Started from oxy-hq/reference-app@81ee8aa `src/components/CoverageRebalanceList.tsx` (INVENTORY
// section 4: 0 copies, no origin named; transcribed there from the HR module design). Kept: one
// row per proposed move inside a titled `Panel`, each row stating the whole move in its own text
// (from, a neutral connector arrow, the destination in the point colour; then a detail line ending
// in the result, also in the point colour, since the result is what a reader decides on) with an
// inline Apply button per row that toggles independently (`aria-pressed`; primary while it is
// still an offer, secondary once applied); an applied row takes the success tint; the panel's
// right slot is a reset once anything is applied, and the reset hands focus to the first row's
// Apply before it unmounts (a keyboard user would otherwise land on <body>); a footnote under the
// list; a sentence for an empty list. New, as a layout with slots: the rows' parts (`from`, `to`,
// `detail`, `result`), the title, the idle mark and every word are the caller's; each Apply is
// described by its row's text (`aria-describedby`), so ten of them are not ten bare "Apply"s.
//
// Not a click-a-row list: the row's only action is committing it, several at once, so the action
// is a named button rather than an overloaded row click.

import { type ReactNode, useId, useRef } from "react";
import { Button } from "../actions";
import { Panel } from "../containers";
import { cx } from "../cx";
import { CheckIcon } from "../icons";
import { ui } from "../recipes";
import { ArrowCounterClockwiseIcon, ArrowRightIcon } from "./icons";

export interface CoverageRebalanceListProps<R> {
  /** The panel's title: "Rebalance moves · Monday". */
  title: ReactNode;
  /** The moves, ranked. */
  recs: readonly R[];
  recKey: (rec: R) => string;
  /** Where the move starts: ordinary ink, the state before it. */
  from: (rec: R) => ReactNode;
  /** Where it ends: the point colour, part of the result. */
  to: (rec: R) => ReactNode;
  /** The detail line: "Lunch 11:00-14:00 · move 6h crew · save". */
  detail?: (rec: R) => ReactNode;
  /** The figure the move changes, at the end of the detail line, in the point colour. */
  result?: (rec: R) => ReactNode;
  /** Keys of the applied moves. */
  applied: ReadonlySet<string>;
  onToggle: (key: string) => void;
  /** Un-applies every move. Without it there is no reset. */
  onReset?: () => void;
  /** Default "Apply". */
  applyLabel?: ReactNode;
  /** Default "Applied", with a check. */
  appliedLabel?: ReactNode;
  /** Default "Reset". */
  resetLabel?: ReactNode;
  /** In the panel's right slot while nothing is applied: a decorative mark, or a count. */
  idle?: ReactNode;
  /** The note under the list: what applying does. */
  foot?: ReactNode;
  /** Said when there is no move to offer. Default "Nothing to rebalance." */
  empty?: ReactNode;
  className?: string;
}

export function CoverageRebalanceList<R>({
  title,
  recs,
  recKey,
  from,
  to,
  detail,
  result,
  applied,
  onToggle,
  onReset,
  applyLabel = "Apply",
  appliedLabel = "Applied",
  resetLabel = "Reset",
  idle,
  foot,
  empty = "Nothing to rebalance.",
  className
}: CoverageRebalanceListProps<R>) {
  const firstApply = useRef<HTMLButtonElement>(null);
  const base = useId();
  const appliedCount = recs.filter((r) => applied.has(recKey(r))).length;
  const right =
    appliedCount > 0 && onReset ? (
      <Button
        variant='text'
        icon={<ArrowCounterClockwiseIcon className={ui.iconInline} />}
        onClick={() => {
          // The reset unmounts with the last applied move: hand focus on first.
          firstApply.current?.focus();
          onReset();
        }}
      >
        {resetLabel}
      </Button>
    ) : (
      idle
    );

  return (
    <Panel title={title} right={right} className={cx("oxy-ui-rebalance-list", className)}>
      {recs.length === 0 ? (
        <p className={cx(ui.note, "oxy-ui-rebalance-list__empty")}>{empty}</p>
      ) : (
        <div className='oxy-ui-rebalance-list__body'>
          <ul className='oxy-ui-rebalance-list__rows'>
            {recs.map((r, i) => {
              const key = recKey(r);
              const isApplied = applied.has(key);
              const text = `${base}-${i}`;
              return (
                <li
                  key={key}
                  className={cx(
                    "oxy-ui-rebalance-list__row",
                    isApplied && "oxy-ui-rebalance-list__row--applied"
                  )}
                >
                  <div id={text} className='oxy-ui-rebalance-list__text'>
                    <div className={cx(ui.text.sm, "oxy-ui-rebalance-list__move")}>
                      <span className='oxy-ui-rebalance-list__from'>{from(r)}</span>
                      <ArrowRightIcon
                        className={cx(ui.iconInline, "oxy-ui-rebalance-list__arrow")}
                      />
                      <span className='oxy-ui-rebalance-list__to'>{to(r)}</span>
                    </div>
                    {(detail || result) && (
                      <div className={cx(ui.note, ui.tabularNums)}>
                        {detail?.(r)}
                        {result && (
                          <>
                            {detail ? " " : null}
                            <span className='oxy-ui-rebalance-list__result'>{result(r)}</span>
                          </>
                        )}
                      </div>
                    )}
                  </div>
                  <Button
                    ref={i === 0 ? firstApply : undefined}
                    variant={isApplied ? "secondary" : "primary"}
                    aria-pressed={isApplied}
                    aria-describedby={text}
                    icon={isApplied ? <CheckIcon className={ui.iconInline} /> : undefined}
                    onClick={() => onToggle(key)}
                  >
                    {isApplied ? appliedLabel : applyLabel}
                  </Button>
                </li>
              );
            })}
          </ul>
          {foot != null && <p className={cx(ui.note, "oxy-ui-rebalance-list__foot")}>{foot}</p>}
        </div>
      )}
    </Panel>
  );
}
