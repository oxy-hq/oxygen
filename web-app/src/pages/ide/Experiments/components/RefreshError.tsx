import ErrorAlert from "@/components/ui/ErrorAlert";
import { Button } from "@/components/ui/shadcn/button";

/**
 * A failed background refetch while the last good list is still on screen: a
 * small banner above the page, so what is mounted (typed rows, a pending
 * request) is not thrown away for the sake of telling the user the read failed.
 */
export default function RefreshError({
  message,
  onRetry
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <div className='px-4 pt-2' data-testid='experiments-refresh-error'>
      <ErrorAlert
        title='Could not refresh experiments'
        message={message}
        actions={
          <Button variant='outline' size='sm' onClick={onRetry}>
            Retry
          </Button>
        }
      />
    </div>
  );
}
