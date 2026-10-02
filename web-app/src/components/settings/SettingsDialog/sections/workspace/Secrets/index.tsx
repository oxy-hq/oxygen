import { KeyRound, Plus } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import { CreateSecretDialog } from "@/components/settings/secrets/CreateSecretDialog";
import { UnifiedSecretsTable } from "@/components/settings/secrets/UnifiedSecretsTable";
import { Button } from "@/components/ui/shadcn/button";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";

const Secrets: React.FC = () => {
  const [isCreateDialogOpen, setIsCreateDialogOpen] = useState(false);

  return (
    <CanWorkspaceAdmin
      fallback={<NoAccessNotice>You need workspace admin access to manage secrets.</NoAccessNotice>}
    >
      <div className='flex flex-col gap-5'>
        <SectionHeader
          icon={KeyRound}
          title='Secrets'
          actions={
            <Button size='sm' variant='outline' onClick={() => setIsCreateDialogOpen(true)}>
              <Plus />
              Create
            </Button>
          }
        />

        <UnifiedSecretsTable />

        <CreateSecretDialog
          open={isCreateDialogOpen}
          onOpenChange={setIsCreateDialogOpen}
          // No toast here: the mutation hooks say so already.
          onSecretCreated={() => setIsCreateDialogOpen(false)}
        />
      </div>
    </CanWorkspaceAdmin>
  );
};

export default Secrets;
