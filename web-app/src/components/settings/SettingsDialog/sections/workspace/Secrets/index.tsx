import { KeyRound, Plus } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import { AppsMissingSecrets } from "@/components/settings/secrets/AppsMissingSecrets";
import { CreateSecretDialog } from "@/components/settings/secrets/CreateSecretDialog";
import { UnifiedSecretsTable } from "@/components/settings/secrets/UnifiedSecretsTable";
import { Button } from "@/components/ui/shadcn/button";
import useSecrets from "@/hooks/api/secrets/useSecrets";
import { useSharedSecrets } from "@/hooks/api/secrets/useSharedSecrets";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";

/** What the create dialog opens with: blank, or a key an app is missing. */
type CreateFor = { name?: string; appIds?: string[] };

const SecretsBody: React.FC = () => {
  const [createFor, setCreateFor] = useState<CreateFor | null>(null);
  const { data: secrets } = useSecrets();
  const { data: sharing } = useSharedSecrets();

  return (
    <div className='flex flex-col gap-5'>
      <SectionHeader
        icon={KeyRound}
        title='Secrets'
        actions={
          <Button size='sm' variant='outline' onClick={() => setCreateFor({})}>
            <Plus />
            Create
          </Button>
        }
      />
      <p className='-mt-2 text-muted-foreground text-sm'>
        Store a credential once and share it with any of this workspace's apps — each reads it as{" "}
        <code>ctx.env.NAME</code>, and changing it here updates every one of them.
      </p>

      <AppsMissingSecrets
        sharing={sharing}
        secrets={secrets?.secrets ?? []}
        onCreate={(name, appId) => setCreateFor({ name, appIds: [appId] })}
      />

      <UnifiedSecretsTable />

      <CreateSecretDialog
        open={createFor !== null}
        onOpenChange={(open) => !open && setCreateFor(null)}
        initialName={createFor?.name}
        initialAppIds={createFor?.appIds}
        // No toast here: the dialog and its mutation hooks say so already.
        onSecretCreated={() => setCreateFor(null)}
      />
    </div>
  );
};

const Secrets: React.FC = () => (
  <CanWorkspaceAdmin
    fallback={<NoAccessNotice>You need workspace admin access to manage secrets.</NoAccessNotice>}
  >
    <SecretsBody />
  </CanWorkspaceAdmin>
);

export default Secrets;
