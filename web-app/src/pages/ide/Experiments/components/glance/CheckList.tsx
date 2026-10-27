import type { ReactNode } from "react";
import { type Tone, ToneBadge, ToneIcon } from "@/pages/ide/Experiments/components/glance/tone";

/** One line of a checklist: a plain title, a short coloured status, and the sentences behind it. */
export interface CheckItem {
  id: string;
  /** Test id for the row's body, when a caller needs a stable one. */
  testId?: string;
  title: string;
  status: string;
  tone: Tone;
  lines: ReactNode[];
}

export function CheckRow({ check }: { check: CheckItem }) {
  return (
    <li
      className='grid grid-cols-[auto_1fr] gap-x-3 px-4 py-3'
      data-testid={`experiment-check-${check.id}`}
      data-tone={check.tone}
    >
      <ToneIcon tone={check.tone} className='mt-0.5 size-4' />
      <div className='min-w-0' data-testid={check.testId}>
        <div className='flex flex-wrap items-center justify-between gap-2'>
          <span className='font-medium text-sm'>{check.title}</span>
          <ToneBadge tone={check.tone}>{check.status}</ToneBadge>
        </div>
        {check.lines.map((line, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: lines are static per render and may repeat
          <p key={i} className='mt-1 whitespace-pre-wrap text-muted-foreground text-xs'>
            {line}
          </p>
        ))}
      </div>
    </li>
  );
}

/** A divided list of check rows. */
export function CheckList({ checks }: { checks: CheckItem[] }) {
  return (
    <ul className='divide-y'>
      {checks.map((c) => (
        <CheckRow key={c.id} check={c} />
      ))}
    </ul>
  );
}
