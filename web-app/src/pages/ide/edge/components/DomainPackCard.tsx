import { AlertCircle, ArrowUpCircle, Edit, Loader2, Shuffle } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import useActivePack from "@/hooks/api/cameras/useActivePack";
import useStarterUpdates from "@/hooks/api/cameras/useStarterUpdates";
import type { PackStarterUpdate } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import PackEditorDialog from "./PackEditorDialog";
import StarterUpdateDialog from "./StarterUpdateDialog";
import SwitchPackDialog from "./SwitchPackDialog";

/**
 * Read-only summary card for the workspace's active domain pack.
 * Lists the active pack's roles + variable keys, surfaces the
 * starter tracking info (which Oxy upstream is forked, what
 * version), and opens the editor.
 *
 * For the typical workspace (auto-seeded restaurant pack, never
 * edited) the card is mostly a "yes, packs exist, here's what
 * roles your operators can pick from." For workspaces that have
 * forked + edited a pack, the `locally_modified` badge marks the
 * pack as diverged from the upstream Oxy ships — useful when the
 * future starter-updates diff modal asks "merge upstream changes?"
 */
const DomainPackCard: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const { data: pack, isLoading, error } = useActivePack(workspace?.id);
  const { data: updates = [] } = useStarterUpdates(workspace?.id);
  const [editorOpen, setEditorOpen] = useState(false);
  const [switchOpen, setSwitchOpen] = useState(false);
  const [updateDialog, setUpdateDialog] = useState<PackStarterUpdate | null>(null);

  if (isLoading) {
    return (
      <div className='flex items-center gap-2 border border-border p-4 text-muted-foreground text-sm'>
        <Loader2 className='h-4 w-4 animate-spin' />
        Loading active pack…
      </div>
    );
  }

  if (error || !pack) {
    return (
      <div className='flex items-start gap-2 border border-destructive/40 bg-destructive/5 p-4 text-destructive text-sm'>
        <AlertCircle className='mt-0.5 h-4 w-4 shrink-0' />
        <div>
          <div className='font-medium'>Couldn't load the active pack.</div>
          <div className='text-destructive/80 text-xs'>
            {error instanceof Error ? error.message : "unknown error"}
          </div>
        </div>
      </div>
    );
  }

  const variableKeys = Object.keys(pack.body.variables ?? {});
  // Find the pending update for the currently-active pack (if any).
  // Other forked-but-inactive packs may also have updates — the
  // SwitchPackDialog can grow a per-row indicator later if that
  // becomes a common case.
  const activePackUpdate = updates.find((u) => u.pack_key === pack.pack_key) ?? null;

  return (
    <div className='flex flex-col gap-3 border border-border p-4'>
      {activePackUpdate && (
        <div className='flex items-center justify-between gap-3 border border-primary/40 bg-primary/5 p-3'>
          <div className='flex items-start gap-2'>
            <ArrowUpCircle className='mt-0.5 h-4 w-4 shrink-0 text-primary' />
            <div className='flex flex-col gap-0.5'>
              <span className='font-medium text-sm'>
                Oxy update available · {activePackUpdate.current_version} →{" "}
                {activePackUpdate.available_version}
              </span>
              <span className='text-muted-foreground text-xs'>
                Review per-role changes from upstream before applying. Defaults to "keep mine"; you
                opt into upstream changes row by row.
              </span>
            </div>
          </div>
          <Button size='sm' onClick={() => setUpdateDialog(activePackUpdate)}>
            Review update
          </Button>
        </div>
      )}

      <div className='flex items-start justify-between gap-3'>
        <div className='flex flex-col gap-1'>
          <div className='flex items-center gap-2'>
            <span className='font-medium'>{pack.name}</span>
            <Badge variant='secondary'>
              {pack.starter_key}@{pack.starter_version}
            </Badge>
            {pack.locally_modified && <Badge variant='outline'>Locally edited</Badge>}
          </div>
          {pack.description && (
            <span className='text-muted-foreground text-sm'>{pack.description}</span>
          )}
        </div>
        <div className='flex items-center gap-2'>
          <Button size='sm' variant='outline' onClick={() => setSwitchOpen(true)}>
            <Shuffle />
            Switch pack
          </Button>
          <Button size='sm' variant='outline' onClick={() => setEditorOpen(true)}>
            <Edit />
            Edit pack
          </Button>
        </div>
      </div>

      <div className='grid gap-3 sm:grid-cols-2'>
        <div className='flex flex-col gap-1'>
          <span className='text-muted-foreground text-xs uppercase tracking-wide'>Roles</span>
          {pack.body.roles.length === 0 ? (
            <span className='text-muted-foreground text-sm italic'>None defined</span>
          ) : (
            <ul className='flex flex-col gap-1 text-sm'>
              {pack.body.roles.map((r) => (
                <li key={r.role_key} className='flex items-baseline justify-between gap-2'>
                  <span>
                    <span className='font-medium'>{r.label}</span>{" "}
                    <code className='text-muted-foreground text-xs'>{r.role_key}</code>
                  </span>
                  <span className='truncate text-muted-foreground text-xs'>{r.hint}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className='flex flex-col gap-1'>
          <span className='text-muted-foreground text-xs uppercase tracking-wide'>Variables</span>
          {variableKeys.length === 0 ? (
            <span className='text-muted-foreground text-sm italic'>None defined</span>
          ) : (
            <ul className='flex flex-wrap gap-1 text-sm'>
              {variableKeys.map((k) => (
                <li key={k}>
                  <code className='bg-muted px-1.5 py-0.5 text-muted-foreground text-xs'>{k}</code>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      <PackEditorDialog pack={pack} open={editorOpen} onOpenChange={setEditorOpen} />
      <SwitchPackDialog open={switchOpen} onOpenChange={setSwitchOpen} />
      <StarterUpdateDialog
        update={updateDialog}
        open={!!updateDialog}
        onOpenChange={(open) => {
          if (!open) setUpdateDialog(null);
        }}
      />
    </div>
  );
};

export default DomainPackCard;
