import { Check, Loader2, Plus } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import useActivatePack from "@/hooks/api/cameras/useActivatePack";
import useForkPack from "@/hooks/api/cameras/useForkPack";
import usePacks from "@/hooks/api/cameras/usePacks";
import useStarterPacks from "@/hooks/api/cameras/useStarterPacks";
import type { DomainPackSummary, StarterPackSummary } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/**
 * Two-section picker for switching the workspace's active domain
 * pack. Top half: already-forked packs with Activate (or "Current"
 * for the active one). Bottom half: Oxy starters the workspace
 * hasn't forked yet, with "Use this pack" — forks AND activates in
 * one click.
 *
 * Hiding already-forked starters from the bottom list avoids
 * confusing the operator about why a Fork might 409 on a key
 * that's already in their workspace. The Activate button on the
 * existing pack covers that case anyway.
 *
 * Fork-then-activate is atomic on the backend (transaction in
 * `packs::create_from_starter`). No half-state to worry about
 * here.
 */
const SwitchPackDialog: React.FC<Props> = ({ open, onOpenChange }) => {
  const { workspace } = useCurrentWorkspace();
  const { data: packs = [], isLoading: packsLoading } = usePacks(workspace?.id);
  const { data: starters = [], isLoading: startersLoading } = useStarterPacks(workspace?.id);
  const fork = useForkPack(workspace?.id);
  const activate = useActivatePack(workspace?.id);

  // Filter the starter library down to keys the workspace hasn't
  // forked yet — keeps the picker focused on actionable choices.
  const forkedKeys = useMemo(() => new Set(packs.map((p) => p.starter_key)), [packs]);
  const availableStarters = useMemo(
    () => starters.filter((s) => !forkedKeys.has(s.key)),
    [starters, forkedKeys]
  );

  const onActivate = async (packKey: string) => {
    try {
      await activate.mutateAsync({ packKey });
      toast.success(`Activated ${packKey}`);
      onOpenChange(false);
    } catch (err) {
      toast.error(`Could not activate: ${errMsg(err)}`);
    }
  };

  const onFork = async (starterKey: string) => {
    try {
      await fork.mutateAsync({ starterKey });
      toast.success(`Forked ${starterKey}`);
      onOpenChange(false);
    } catch (err) {
      toast.error(`Could not fork: ${errMsg(err)}`);
    }
  };

  const isBusy = fork.isPending || activate.isPending;
  const loading = packsLoading || startersLoading;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-3xl`}>
        <DialogHeader>
          <DialogTitle>Switch domain pack</DialogTitle>
          <DialogDescription>
            Switch the active pack used by VLM prompts, or fork a new pack from Oxy's curated
            starter library.
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className='flex items-center justify-center py-10 text-muted-foreground text-sm'>
            <Loader2 className='mr-2 h-4 w-4 animate-spin' />
            Loading packs…
          </div>
        ) : (
          <div className='flex flex-col gap-6'>
            <ForkedPacksSection packs={packs} busy={isBusy} onActivate={onActivate} />
            <StartersSection starters={availableStarters} busy={isBusy} onFork={onFork} />
          </div>
        )}

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)} disabled={isBusy}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default SwitchPackDialog;

// ── Sections ──────────────────────────────────────────────────────

const ForkedPacksSection: React.FC<{
  packs: DomainPackSummary[];
  busy: boolean;
  onActivate: (packKey: string) => void;
}> = ({ packs, busy, onActivate }) => (
  <section>
    <h3 className='mb-2 font-medium text-sm'>This workspace's packs</h3>
    <div className='flex flex-col gap-2'>
      {packs.map((p) => (
        <div
          key={p.id}
          className={
            p.is_active
              ? "flex items-start justify-between gap-3 border border-primary bg-primary/5 p-3"
              : "flex items-start justify-between gap-3 border border-border p-3"
          }
        >
          <div className='flex flex-col gap-1'>
            <div className='flex items-center gap-2'>
              <span className='font-medium'>{p.name}</span>
              <Badge variant='secondary'>
                {p.starter_key}@{p.starter_version}
              </Badge>
              {p.locally_modified && <Badge variant='outline'>Edited</Badge>}
            </div>
            {p.description && (
              <span className='text-muted-foreground text-xs'>{p.description}</span>
            )}
          </div>
          {p.is_active ? (
            <Badge variant='default' className='gap-1'>
              <Check className='h-3 w-3' />
              Active
            </Badge>
          ) : (
            <Button
              size='sm'
              variant='outline'
              onClick={() => onActivate(p.pack_key)}
              disabled={busy}
            >
              Activate
            </Button>
          )}
        </div>
      ))}
    </div>
  </section>
);

const StartersSection: React.FC<{
  starters: StarterPackSummary[];
  busy: boolean;
  onFork: (starterKey: string) => void;
}> = ({ starters, busy, onFork }) => (
  <section>
    <h3 className='mb-2 font-medium text-sm'>Fork from a starter</h3>
    {starters.length === 0 ? (
      <p className='text-muted-foreground text-sm italic'>
        All published starters are already forked in this workspace.
      </p>
    ) : (
      <div className='flex flex-col gap-2'>
        {starters.map((s) => (
          <div
            key={s.key}
            className='flex items-start justify-between gap-3 border border-border p-3'
          >
            <div className='flex flex-col gap-1'>
              <div className='flex items-center gap-2'>
                <span className='font-medium'>{s.name}</span>
                <Badge variant='secondary'>v{s.version}</Badge>
              </div>
              <span className='text-muted-foreground text-xs'>{s.description}</span>
            </div>
            <Button size='sm' onClick={() => onFork(s.key)} disabled={busy}>
              <Plus />
              Use this pack
            </Button>
          </div>
        ))}
      </div>
    )}
  </section>
);

const errMsg = (err: unknown) => (err instanceof Error ? err.message : "unknown error");
