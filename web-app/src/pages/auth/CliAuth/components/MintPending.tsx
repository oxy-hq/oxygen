import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";

interface Props {
  /** What the request is read against could not be loaded. */
  failed: boolean;
  onRetry: () => void;
  /** Said to a screen reader while it loads. */
  loadingLabel: string;
  /** Why the request can't be checked, when loading failed. */
  failedText: string;
}

/**
 * In place of the request while what it is read against is still loading, or couldn't be: a
 * request that hasn't been checked is never shown as one that can be approved.
 */
const MintPending: React.FC<Props> = ({ failed, onRetry, loadingLabel, failedText }) =>
  failed ? (
    <div
      className='mt-6 flex flex-col items-start gap-3 py-2 text-sm leading-5.5'
      data-testid='cli-auth-mint-options-error'
    >
      <p>{failedText}</p>
      <Button variant='outline' size='sm' onClick={onRetry}>
        Try again
      </Button>
    </div>
  ) : (
    <div className='mt-6 flex flex-col gap-4 py-2' data-testid='cli-auth-mint-loading'>
      <span className='sr-only'>{loadingLabel}</span>
      <Skeleton className='h-5 w-48' />
      <Skeleton className='h-5 w-80 max-w-full' />
      <Skeleton className='h-5 w-64' />
    </div>
  );

export default MintPending;
