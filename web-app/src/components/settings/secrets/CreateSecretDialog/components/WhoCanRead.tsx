import { CopyFromWorkspace } from "@/components/settings/secrets/CopyFromWorkspace";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { AppSecretNeeds } from "@/types/sharedSecrets";
import { AppChecklist } from "../../AppChecklist";

/** Where a new secret is stored, and so who can read it. */
export type Scope =
  /** One workspace secret, shared with any number of apps. */
  | { kind: "workspace"; appIds: Set<string> }
  /** One app's own secret (`apps/<app_id>/<KEY>`), read by that app alone. */
  | { kind: "app"; appId: string };

/**
 * "Who can read it" for a new secret. **Workspace** is the default and the
 * answer to "several apps need the same key": it is stored once, shared with the
 * ticked apps, and rotated in one place. **One app only** is for a value that
 * must differ per app — a second app wanting another `API_KEY` than the
 * workspace's — and keeps the copy-a-workspace-value shortcut.
 */
export const WhoCanRead = ({
  apps,
  name,
  scope,
  onScope,
  onCopy
}: {
  apps: AppSecretNeeds[];
  /** The name being typed — the key each app will read. */
  name: string;
  scope: Scope;
  onScope: (scope: Scope) => void;
  onCopy: (value: string) => void;
}) => {
  if (apps.length === 0) return null;
  return (
    <div className='grid gap-2'>
      <Label>Who can read it</Label>
      <ToggleGroup
        type='single'
        variant='outline'
        size='sm'
        value={scope.kind}
        onValueChange={(kind) => {
          if (kind === "workspace") onScope({ kind, appIds: new Set() });
          if (kind === "app") onScope({ kind, appId: apps[0].app_id });
        }}
        className='w-full'
      >
        <ToggleGroupItem
          value='workspace'
          className='flex-1 text-xs'
          data-testid='secret-scope-workspace'
        >
          Workspace &amp; shared apps
        </ToggleGroupItem>
        <ToggleGroupItem value='app' className='flex-1 text-xs' data-testid='secret-scope-app'>
          One app only
        </ToggleGroupItem>
      </ToggleGroup>

      {scope.kind === "workspace" ? (
        <>
          <p className='text-muted-foreground text-xs'>
            Stored once. Each app you tick reads it as <code>ctx.env.{name.trim() || "NAME"}</code>,
            and changing it later updates all of them.
          </p>
          <AppChecklist
            apps={apps}
            secretName={name}
            selected={scope.appIds}
            onChange={(appIds) => onScope({ kind: "workspace", appIds })}
          />
        </>
      ) : (
        <>
          <Select value={scope.appId} onValueChange={(appId) => onScope({ kind: "app", appId })}>
            <SelectTrigger aria-label='App'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {apps.map((app) => (
                <SelectItem key={app.app_id} value={app.app_id}>
                  {app.app_name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className='text-muted-foreground text-xs'>
            Only this app can read it, as <code>ctx.env.{name.trim() || "KEY"}</code>. It wins over
            a shared workspace secret of the same name.
          </p>
          <CopyFromWorkspace name={name} onCopy={onCopy} />
        </>
      )}
    </div>
  );
};
