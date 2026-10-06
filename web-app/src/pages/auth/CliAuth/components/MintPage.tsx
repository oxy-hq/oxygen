import type React from "react";
import { OxyMark } from "@/components/OxyMark";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type { CliAuthStatus } from "./CliAuthCard";

interface Props {
  /** Where the mint stands, for a test or a flow to read off the page. */
  status: CliAuthStatus;
  title: string;
  /** The sentence under the title: what is asked and by whom, or what happens next. */
  lead: React.ReactNode;
  /** The request and its buttons. Left out for a state with nothing to read or do. */
  children?: React.ReactNode;
}

/**
 * The page of a sandbox agent mint, in every state it passes through: checking the session, the
 * approval itself, approved and declined. One left-aligned sheet under the product's mark, with
 * no card around it, so the page doesn't change shape as the request moves on.
 *
 * The page scrolls itself: `html` and `body` are fixed and clip, so a sheet taller than a short
 * window would otherwise lose its buttons.
 */
const MintPage: React.FC<Props> = ({ status, title, lead, children }) => (
  <div className='h-svh w-full overflow-y-auto bg-background text-foreground'>
    <main
      className='mx-auto w-full max-w-160 px-6 pt-8 pb-10 sm:px-10 md:pt-20'
      data-testid='cli-auth-card'
      data-status={status}
    >
      <div className='flex items-center gap-2 font-semibold leading-5 tracking-tight'>
        <OxyMark className='-ml-0.5 size-5' />
        Oxygen
      </div>
      {/* The spinner follows the title, so every state's title starts on the same edge. */}
      <h1 className='mt-12 flex items-center gap-2.5 text-balance font-semibold text-xl leading-7 tracking-tight'>
        {title}
        {status === "working" && <Spinner className='size-4 shrink-0 text-muted-foreground' />}
      </h1>
      <p className='mt-1.5 text-pretty text-muted-foreground text-sm leading-5.5'>{lead}</p>
      {children}
    </main>
  </div>
);

export default MintPage;
