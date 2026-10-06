import { Fingerprint, Plus } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import SectionHeader from "../../../components/SectionHeader";
import CreateTokenDialog from "./components/CreateTokenDialog";
import LegacyKeysPointer from "./components/LegacyKeysPointer";
import TokenSecretDialog, { type SecretReveal } from "./components/TokenSecretDialog";
import TokenTable from "./components/TokenTable";

/**
 * Account → Personal access tokens: the caller's own tokens, across every organization and
 * workspace. Needs no workspace, so it works before one is selected. Legacy API keys are not
 * here; one line under the table says where they are.
 */
const PersonalTokens: React.FC = () => {
  const [createOpen, setCreateOpen] = useState(false);
  // The only copy of a new secret the browser ever holds. Cleared when the dialog is dismissed.
  const [reveal, setReveal] = useState<SecretReveal | null>(null);

  return (
    <div className='flex flex-col gap-5' data-testid='settings-account-tokens'>
      <SectionHeader
        icon={Fingerprint}
        title='Personal access tokens'
        description='A token signs in as you from oxyc, a script or CI. Give it everything you can reach, or only the workspaces you pick.'
        actions={
          <Button
            size='sm'
            variant='outline'
            onClick={() => setCreateOpen(true)}
            data-testid='account-token-create-button'
          >
            <Plus />
            Create token
          </Button>
        }
      />

      <div className='flex flex-col gap-2'>
        <TokenTable onRegenerated={(result) => setReveal({ ...result, reason: "regenerated" })} />
        <LegacyKeysPointer />
      </div>

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
