import { ArrowRight, KeyRound, Plus } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import { AppsMissingSecrets } from "@/components/settings/secrets/AppsMissingSecrets";
import { CreateSecretDialog } from "@/components/settings/secrets/CreateSecretDialog";
import { UnifiedSecretsTable } from "@/components/settings/secrets/UnifiedSecretsTable";
import { buildRows, inScope } from "@/components/settings/secrets/UnifiedSecretsTable/rows";
import type { SecretsScope } from "@/components/settings/secrets/UnifiedSecretsTable/types";
import { Button } from "@/components/ui/shadcn/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { useAuth } from "@/contexts/AuthContext";
import useSecrets from "@/hooks/api/secrets/useSecrets";
import { useSharedSecrets } from "@/hooks/api/secrets/useSharedSecrets";
import useSettingsDialog from "@/stores/useSettingsDialog";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";
import type { SecretsTab } from "../../../tabs";

/** What the create dialog opens with: blank, or a key an app is missing. */
type CreateFor = { name?: string; appIds?: string[] };

const BLURB: Record<SecretsScope, React.ReactNode> = {
  workspace: (
    <>
      Stored once and shared with whichever apps you choose — each reads it as{" "}
      <code>ctx.env.NAME</code>, and changing it here reaches every one of them. Your organization's
      databases, integrations and agents read these too.
    </>
  ),
  app: (
    <>
      A value one app keeps for itself, stored under that app and readable by nothing else. An app's
      own value of a key wins over a shared one, so this is where a single app differs from the
      rest.
    </>
  )
};

/**
 * Tab id (a link names it: `?settings=organization.secrets.app`) to the scope the tables read.
 * The tab a person sees is "Shared"; the code's scope is still `workspace`, after the row's
 * storage: a secret stored on the org's workspace rather than under one app.
 */
const SCOPE_OF: Record<SecretsTab, SecretsScope> = { shared: "workspace", app: "app" };
const TAB_OF: Record<SecretsScope, SecretsTab> = { workspace: "shared", app: "app" };

const SecretsBody: React.FC = () => {
  const requested = useSettingsDialog((s) => s.tab);
  const setRequested = useSettingsDialog((s) => s.setTab);
  const openSettings = useSettingsDialog((s) => s.open);
  const { isLocalMode } = useAuth();
  const tab: SecretsScope = SCOPE_OF[requested as SecretsTab] ?? "workspace";
  const setTab = (scope: SecretsScope) => setRequested(TAB_OF[scope]);
  const [createFor, setCreateFor] = useState<CreateFor | null>(null);
  const { data: secrets } = useSecrets();
  const { data: sharing } = useSharedSecrets();

  // The counts on the tabs, from the same two queries the tables read.
  const counts = useMemo(() => {
    const names = new Map((sharing?.apps ?? []).map((a) => [a.app_id, a.app_name]));
    const rows = buildRows(secrets?.secrets ?? [], sharing?.shared ?? [], names);
    return {
      workspace: inScope(rows, "workspace").length,
      app: inScope(rows, "app").length
    };
  }, [secrets, sharing]);

  const openCreate = (name?: string, appIds?: string[]) => setCreateFor({ name, appIds });

  return (
    <div className='flex flex-col gap-5'>
      <SectionHeader
        icon={KeyRound}
        title='Secrets'
        description={
          <>
            Values Oxygen keeps for your apps, agents and databases, such as another service's API
            key.
            {/* Local mode has no sign-in, so no API tokens section to point at. */}
            {!isLocalMode && (
              <>
                {" "}
                For a token that lets something call Oxygen itself, see{" "}
                <button
                  type='button'
                  className='inline-flex items-center gap-0.5 font-medium text-foreground underline-offset-2 hover:underline'
                  onClick={() => openSettings("organization.api_tokens")}
                  data-testid='secrets-api-tokens-link'
                >
                  API tokens
                  <ArrowRight className='size-3' aria-hidden />
                </button>
                .
              </>
            )}
          </>
        }
        actions={
          <Button size='sm' variant='outline' onClick={() => openCreate()}>
            <Plus />
            Create
          </Button>
        }
      />

      <AppsMissingSecrets
        sharing={sharing}
        secrets={secrets?.secrets ?? []}
        onCreate={(name, appId) => openCreate(name, [appId])}
      />

      {/* Two tabs rather than one list with a kind filter: a workspace secret
          and an app's own secret are different things with different verbs, and
          a filter chip made them look like two views of one list. */}
      <Tabs value={tab} onValueChange={(value) => setTab(value as SecretsScope)}>
        <TabsList data-testid='secrets-tabs'>
          <TabsTrigger value='workspace' data-testid='secrets-tab-shared'>
            Shared
            <span className='ml-1.5 text-muted-foreground tabular-nums'>{counts.workspace}</span>
          </TabsTrigger>
          <TabsTrigger value='app' data-testid='secrets-tab-app'>
            One app
            <span className='ml-1.5 text-muted-foreground tabular-nums'>{counts.app}</span>
          </TabsTrigger>
        </TabsList>

        {(["workspace", "app"] as const).map((scope) => (
          <TabsContent key={scope} value={scope} className='flex flex-col gap-3'>
            <p className='text-muted-foreground text-sm'>{BLURB[scope]}</p>
            <UnifiedSecretsTable scope={scope} onCreateFor={openCreate} />
          </TabsContent>
        ))}
      </Tabs>

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
    fallback={<NoAccessNotice>You need admin access to manage secrets.</NoAccessNotice>}
  >
    <SecretsBody />
  </CanWorkspaceAdmin>
);

export default Secrets;
