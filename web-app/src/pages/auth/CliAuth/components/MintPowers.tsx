import { Check, type LucideIcon, X } from "lucide-react";
import type React from "react";
import { sandboxAgentPowers } from "@/libs/sandboxAgentToken";
import { cn } from "@/libs/shadcn/utils";

const POWERS = sandboxAgentPowers("these apps");

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
          {act}
        </li>
      ))}
    </ul>
  </div>
);

/**
 * What the token can and cannot do, as two lists to scan: a consent is checked line by line, not
 * read. The second column starts on the edge the app names and the end time sit on.
 */
const MintPowers: React.FC = () => (
  <div
    className='flex flex-col gap-x-4 gap-y-4 py-2 text-sm leading-5 sm:flex-row'
    data-testid='cli-auth-mint-powers'
  >
    <Column title='Can' acts={POWERS.can} icon={Check} className='sm:w-64 sm:shrink-0' />
    <Column
      title='Cannot'
      acts={POWERS.cannot}
      icon={X}
      className='min-w-0 flex-1'
      iconClassName='text-muted-foreground'
    />
  </div>
);

export default MintPowers;
