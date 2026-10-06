import type React from "react";
import { Separator } from "@/components/ui/shadcn/separator";
import { sandboxAgentSummary } from "@/libs/sandboxAgentToken";

const SUMMARY = sandboxAgentSummary("the apps you pick");

const Line: React.FC<{ label: string; children: string }> = ({ label, children }) => (
  <div className='flex flex-col py-1 sm:flex-row'>
    <dt className='w-24 shrink-0 text-muted-foreground'>{label}</dt>
    <dd className='min-w-0 flex-1 text-pretty'>{children}</dd>
  </div>
);

/**
 * What a sandbox agent token can and can't do, closing the sheet: two sentences on the same two
 * edges as the fields above. The approval page lists the same acts one per line.
 */
const SandboxSummary: React.FC = () => (
  <div className='shrink-0'>
    <Separator className='mt-2 mb-3' />
    <dl data-testid='account-token-sandbox-summary'>
      <Line label='Can'>{SUMMARY.can}</Line>
      <Line label='Cannot'>{SUMMARY.cannot}</Line>
    </dl>
  </div>
);

export default SandboxSummary;
