import { Loader2, RotateCw } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";

/** "Run again": queues a new attempt of the build's checks on staging. It is the
 *  primary action once a run got no answer, and spins while the request is out. */
export const RerunButton = ({
  primary,
  busy,
  disabled,
  onClick
}: {
  primary: boolean;
  /** The request is out. */
  busy: boolean;
  /** There is no build to check. */
  disabled: boolean;
  onClick: () => void;
}) => (
  <Button
    variant={primary ? "default" : "outline"}
    size='sm'
    className='h-6 gap-1 px-2 text-xs'
    disabled={disabled || busy}
    onClick={onClick}
    title='Check this build again on staging.'
    data-testid='build-checks-rerun'
  >
    {busy ? (
      <Loader2 className='size-3 animate-spin motion-reduce:animate-none' />
    ) : (
      <RotateCw className='size-3' />
    )}
    Run again
  </Button>
);
