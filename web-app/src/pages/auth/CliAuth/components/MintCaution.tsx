import type React from "react";

interface Props {
  hostname: string;
  /** The oxyc command that opens this approval, as the person would have typed it. */
  command: string;
}

/** The last thing read before the buttons: the command that should have opened this page. */
const MintCaution: React.FC<Props> = ({ hostname, command }) => (
  <div className='mt-2 text-sm leading-5.5'>
    <p>
      Continue only if you just ran this on <b className='break-words font-medium'>{hostname}</b>
    </p>
    <div className='mt-2 flex h-9 items-center overflow-x-auto whitespace-nowrap rounded-md bg-accent px-3 font-mono'>
      <span aria-hidden='true' className='mr-2.5 select-none text-muted-foreground'>
        $
      </span>
      <code data-testid='cli-auth-mint-command'>{command}</code>
    </div>
  </div>
);

export default MintCaution;
