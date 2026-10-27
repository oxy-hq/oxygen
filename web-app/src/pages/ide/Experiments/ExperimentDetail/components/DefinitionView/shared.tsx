import type { ReactNode } from "react";
import { HintIcon } from "@/pages/ide/Experiments/components/glance/Hinted";

/** A labelled block inside the Definition section; `hint` puts its explanation behind an ⓘ. */
export function Block({
  id,
  title,
  hint,
  children
}: {
  id: string;
  title: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div data-testid={`definition-${id}`} className='space-y-1.5'>
      <h4 className='flex items-center gap-1 font-medium text-muted-foreground text-xs uppercase'>
        {title}
        {hint && <HintIcon hint={hint} testId={`definition-${id}-hint`} />}
      </h4>
      {children}
    </div>
  );
}

/** One labelled fact: a small label over its value. */
export function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className='min-w-0'>
      <div className='text-muted-foreground text-xs'>{label}</div>
      <div className='break-words text-sm'>{children}</div>
    </div>
  );
}

/** A semantic ref or condition, shown as code beside its plain-English wording. */
export function Chip({ children }: { children: ReactNode }) {
  return (
    <code className='break-all rounded bg-muted px-1.5 py-0.5 font-mono text-xs'>{children}</code>
  );
}

/** A short labelled pill: `On · fruit_summary`. */
export function LabelledChip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <span className='inline-flex max-w-full items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs'>
      <span className='text-muted-foreground'>{label}</span>
      <span className='break-all font-mono'>{children}</span>
    </span>
  );
}

export const Facts = ({ children }: { children: ReactNode }) => (
  <div className='grid gap-x-6 gap-y-2 sm:grid-cols-2'>{children}</div>
);
