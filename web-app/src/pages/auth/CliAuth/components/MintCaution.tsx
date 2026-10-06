import type React from "react";

/** The last thing read before the buttons: the command that should have opened this page. */
const MintCaution: React.FC<{ hostname: string }> = ({ hostname }) => (
  <div className='mt-2 text-sm leading-5.5'>
    <p>
      Continue only if you just ran this on <b className='break-all font-medium'>{hostname}</b>
    </p>
    <div className='mt-2 flex h-9 items-center overflow-x-auto whitespace-nowrap rounded-md bg-accent px-3 font-mono'>
      <span aria-hidden='true' className='mr-2.5 select-none text-muted-foreground'>
        $
      </span>
      <code>oxyc tokens create --sandbox-agent</code>
    </div>
  </div>
);

export default MintCaution;
