import type { TokenSummaryLine } from "./summary";

interface Props {
  /** `admin-<area>`: the line is `<area>-summary`. */
  area: string;
  summary: TokenSummaryLine;
}

/** The list in one sentence above its table. What works now is in the foreground. */
export function TokenListSummary({ area, summary }: Props) {
  return (
    <p className='text-muted-foreground text-xs' data-testid={`${area}-summary`}>
      <span className='font-medium text-foreground'>{summary.lead}</span>
      {summary.rest}
    </p>
  );
}
