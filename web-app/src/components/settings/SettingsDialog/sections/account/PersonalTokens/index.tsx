import { Plus } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import CreateTokenDialog from "./components/CreateTokenDialog";
import TokenSecretDialog, { type SecretReveal } from "./components/TokenSecretDialog";
import TokenTable from "./components/TokenTable";

/**
 * The Personal access tokens tab of Organization → API tokens: the caller's own tokens, across
 * every organization. Needs no workspace, so it works before one is selected.
 *
 * It was its own section, Account → Personal access tokens, until every token moved onto one
 * page. The folder stays under `account/` because the admin console and the service-account
 * screens import its helpers by this path.
 */
const PersonalTokens: React.FC = () => {
  const [createOpen, setCreateOpen] = useState(false);
  // The only copy of a new secret the browser ever holds. Cleared when the dialog is dismissed.
  const [reveal, setReveal] = useState<SecretReveal | null>(null);

  return (
    <div className='flex flex-col gap-4' data-testid='settings-account-tokens'>
      <div className='flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between'>
        <p className='text-muted-foreground text-xs'>
          Signs in as you from oxyc, a script or CI, and stops working if you leave. Give it
          everything you can reach, or only the organizations you pick.
        </p>
        <Button
          size='sm'
          variant='outline'
          className='self-start sm:self-auto'
          onClick={() => setCreateOpen(true)}
          data-testid='account-token-create-button'
        >
          <Plus />
          Create token
        </Button>
      </div>

      <TokenTable onRegenerated={(result) => setReveal({ ...result, reason: "regenerated" })} />

      <CreateTokenDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={(result) => setReveal({ ...result, reason: "created" })}
      />
      <TokenSecretDialog reveal={reveal} onDone={() => setReveal(null)} />
    </div>
  );
};

export default PersonalTokens;
