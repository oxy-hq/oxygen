import { CircleAlert } from "lucide-react";
import type React from "react";
import { lifetimeLabel, sandboxExpiry } from "@/libs/sandboxAgentToken";
import { ApiKeyService } from "@/services/api/apiKey";

/**
 * The rows a token request is read in before it is approved, whichever kind of token it asks
 * for: a label, then the value on the edge every other value sits on.
 */
export const Row: React.FC<React.PropsWithChildren<{ label: string }>> = ({ label, children }) => (
  <div className='flex py-2'>
    <dt className='w-24 shrink-0 text-muted-foreground'>{label}</dt>
    <dd className='min-w-0 flex-1'>{children}</dd>
  </div>
);

/**
 * A value that can't be granted, shown as asked and marked where it stands: the mark hangs in
 * the gutter, so the value keeps the edge every other value sits on.
 */
export const Refused: React.FC<React.PropsWithChildren<{ why: string }>> = ({ why, children }) => (
  <>
    <div className='relative font-medium text-destructive'>
      <CircleAlert aria-hidden='true' className='absolute top-1 -left-5.5 size-3.5' />
      <span className='sr-only'>Can't be granted: </span>
      {children}
    </div>
    <p className='mt-0.5'>{why}</p>
  </>
);

/** The name the token will carry. One too long to be granted is shown as asked. */
export const NameValue: React.FC<{ name: string; problem: string | null }> = ({
  name,
  problem
}) => (
  <div className='break-words' data-testid='cli-auth-mint-name'>
    {problem ? (
      <Refused why={problem}>{name}</Refused>
    ) : (
      <span className='font-medium'>{name}</span>
    )}
  </div>
);

interface LifetimeProps {
  /** The `hours` param as oxyc sent it, for the one value that can't be read back as a number. */
  asked: string | null;
  /** `null` when what was sent is no whole number. */
  hours: number | null;
  problem: string | null;
}

/** How long the token would live, and until when. One that can't be granted is shown as asked. */
export const LifetimeValue: React.FC<LifetimeProps> = ({ asked, hours, problem }) => (
  <div data-testid='cli-auth-mint-lifetime'>
    {hours === null || problem ? (
      <Refused why={problem ?? ""}>
        {hours === null ? `"${asked ?? ""}"` : lifetimeLabel(hours)}
      </Refused>
    ) : (
      <div className='flex flex-wrap gap-x-4'>
        <span className='min-w-40 font-medium'>{lifetimeLabel(hours)}</span>
        <span className='text-muted-foreground'>
          until {ApiKeyService.formatDate(sandboxExpiry(hours).toISOString())}
        </span>
      </div>
    )}
  </div>
);
