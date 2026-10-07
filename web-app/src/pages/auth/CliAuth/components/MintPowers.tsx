import { Check, type LucideIcon, X } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";

interface ColumnProps {
  title: string;
  acts: string[];
  icon: LucideIcon;
  className?: string;
  iconClassName?: string;
}

const Column: React.FC<ColumnProps> = ({ title, acts, icon: Icon, className, iconClassName }) => (
  <div className={className}>
    <h2 className='font-medium'>{title}</h2>
    <ul className='mt-1.5'>
      {acts.map((act) => (
        <li key={act} className='flex gap-1.5 py-0.5'>
          <Icon aria-hidden='true' className={cn("mt-1 size-3.5 shrink-0", iconClassName)} />
          {/* Balanced, so an act that wraps never leaves its last word alone on a line. */}
          <span className='text-balance'>{act}</span>
        </li>
      ))}
    </ul>
  </div>
);

interface Props {
  /** What the token may do, one act each, worded to follow "Can". */
  can: string[];
  /** What it may not, worded to follow "Cannot". */
  cannot: string[];
}

/**
 * What the token can and cannot do, as two lists to scan: a consent is checked line by line, not
 * read. The second column starts on the edge the values above it sit on.
 */
const MintPowers: React.FC<Props> = ({ can, cannot }) => (
  <div
    className='flex flex-col gap-x-4 gap-y-4 py-2 text-sm leading-5 sm:flex-row'
    data-testid='cli-auth-mint-powers'
  >
    <Column title='Can' acts={can} icon={Check} className='sm:w-64 sm:shrink-0' />
    <Column
      title='Cannot'
      acts={cannot}
      icon={X}
      className='min-w-0 flex-1'
      iconClassName='text-muted-foreground'
    />
  </div>
);

export default MintPowers;
