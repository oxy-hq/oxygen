import { Info } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";

interface Props {
  /** Opens Account → Personal access tokens. Omit where there is no account to open (local mode). */
  onCreateToken?: () => void;
}

/**
 * Why these keys sit apart, and where new credentials are made. The section has no create button
 * of its own, so this is the way forward from it.
 */
const LegacyKeyNotice: React.FC<Props> = ({ onCreateToken }) => (
  <div
    className='flex flex-col gap-3 rounded-md border bg-muted/40 p-3 text-xs sm:flex-row sm:items-center sm:justify-between'
    data-testid='legacy-api-keys-notice'
  >
    <div className='flex min-w-0 gap-2'>
      <Info className='mt-0.5 size-3.5 shrink-0 text-muted-foreground' aria-hidden />
      <div className='flex flex-col gap-1'>
        <p className='font-medium'>
          A legacy API key reaches everything its owner can, and can't be limited to workspaces.
        </p>
        <p className='text-muted-foreground'>
          The keys below keep working as they always have. For anything new, use an API token.
        </p>
      </div>
    </div>
    {onCreateToken && (
      <Button
        size='sm'
        variant='outline'
        className='shrink-0 self-start sm:self-center'
        onClick={onCreateToken}
        data-testid='legacy-api-keys-create-token-link'
      >
        Create an API token
      </Button>
    )}
  </div>
);

export default LegacyKeyNotice;
