import { KeyRound } from "lucide-react";
import type React from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";

/**
 * What sets an agent token apart from the sandbox agent token the same page approves, said
 * before the request itself. The two requests arrive the same way and their titles differ by
 * one word, so someone who expected the narrow one could approve the wide one on a glance.
 *
 * It is a box where the sandbox sheet has none, in the page's own ink: the difference is in the
 * words and the shape of the sheet, never in a colour. Red stays for what is refused.
 */
const AgentScopeNotice: React.FC = () => (
  <Alert className='mt-6 border-foreground' data-testid='cli-auth-agent-notice'>
    <KeyRound aria-hidden='true' />
    <AlertTitle className='line-clamp-none'>
      This token acts as you, everywhere you can go
    </AlertTitle>
    <AlertDescription>
      <p>It is not a sandbox agent token, which reaches only the sandboxes of the apps it names.</p>
    </AlertDescription>
  </Alert>
);

export default AgentScopeNotice;
