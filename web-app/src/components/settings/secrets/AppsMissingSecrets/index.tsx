import { Plus, Share2, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { useShareSecret } from "@/hooks/api/secrets/useSharedSecrets";
import type { Secret } from "@/types/secret";
import type { SharedSecretsResponse } from "@/types/sharedSecrets";
import { missingByApp } from "./missing";

/**
 * The apps of this workspace whose production build declares a key nothing
 * provides yet — the "Set SLACK_BOT_TOKEN in the app's secrets" an app shows its
 * users, answered where secrets are kept. A key the workspace already holds is
 * one click from shared; a key it does not is one click from a create dialog
 * with the name filled in and the app ticked.
 *
 * Renders nothing when every app has what it declares.
 */
export const AppsMissingSecrets = ({
  sharing,
  secrets,
  onCreate
}: {
  sharing: SharedSecretsResponse | undefined;
  secrets: Secret[];
  onCreate: (name: string, appId: string) => void;
}) => {
  const share = useShareSecret();
  const apps = missingByApp(sharing, secrets);
  if (apps.length === 0) return null;

  return (
    <section
      className='flex flex-col gap-3 rounded-lg border border-warning/30 bg-warning/5 p-3'
      data-testid='secrets-apps-missing'
    >
      <h3 className='flex items-center gap-2 font-medium text-sm'>
        <TriangleAlert className='size-4 text-warning' />
        {apps.length === 1
          ? "An app is missing secrets"
          : `${apps.length} apps are missing secrets`}
      </h3>
      <ul className='flex flex-col gap-2'>
        {apps.map((app) => (
          <li key={app.appId} className='flex flex-col gap-1.5'>
            <span className='text-sm'>{app.appName}</span>
            <div className='flex flex-wrap gap-1.5'>
              {app.keys.map((key) => {
                const existing = key.workspaceSecret;
                return existing ? (
                  <Button
                    key={key.key}
                    size='sm'
                    variant='outline'
                    className='h-7 gap-1.5 font-mono text-xs'
                    disabled={share.isPending}
                    title={`The workspace already has ${key.key}: share it with ${app.appName}`}
                    data-testid={`secrets-missing-share-${key.key}`}
                    onClick={() =>
                      share.mutate({
                        secretId: existing.id,
                        name: key.key,
                        appIds: [...existing.appIds, app.appId]
                      })
                    }
                  >
                    <Share2 className='size-3' />
                    Share {key.key}
                  </Button>
                ) : (
                  <Button
                    key={key.key}
                    size='sm'
                    variant='outline'
                    className='h-7 gap-1.5 font-mono text-xs'
                    title={key.description ?? `Create ${key.key} and share it with ${app.appName}`}
                    data-testid={`secrets-missing-create-${key.key}`}
                    onClick={() => onCreate(key.key, app.appId)}
                  >
                    <Plus className='size-3' />
                    {key.key}
                    {key.required && <span className='text-warning'>required</span>}
                  </Button>
                );
              })}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
};
