import { ArrowDown, ArrowUp, Check, Loader2, Minus, Plus } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
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
import useApplyStarterUpdate from "@/hooks/api/cameras/useApplyStarterUpdate";
import {
  type PackStarterUpdate,
  type RoleDiff,
  type RoleDiffStatus,
  type UpdateAction,
  VARIABLES_ACTION_KEY
} from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  update: PackStarterUpdate | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/**
 * Per-pack starter-update review.
 *
 * Layout: one row per role with status badge + side-by-side
 * "Current" / "Upstream" textboxes (read-only), and a 3-state
 * toggle: Take Oxy's | Keep mine. Variables get the same
 * treatment under the `$variables` sentinel key (matches
 * `crate::service::packs::updates::VARIABLES_ACTION_KEY`).
 *
 * Default behavior (no explicit choice) is "keep mine" — the
 * conservative default. Operators have to opt into taking
 * upstream changes for each row, which avoids the dangerous
 * "click Apply and silently rewrite prompts you'd customized."
 *
 * Submit fires `POST /camera-packs/{key}/apply-update`. Backend
 * renders the merged body against a dummy context first; a bad
 * merge (operator took an upstream prompt that references a
 * variable they kept removed) returns 400 + toast, dialog stays
 * open with the same action map so the operator can fix.
 */
const StarterUpdateDialog: React.FC<Props> = ({ update, open, onOpenChange }) => {
  const { workspace } = useCurrentWorkspace();
  const mutation = useApplyStarterUpdate(workspace?.id);
  // Per-key action map. Missing entries → KeepMine on the
  // backend, which matches our "conservative default" stance.
  const [actions, setActions] = useState<Record<string, UpdateAction>>({});

  // Reset when the update prop changes so opening on pack A then
  // pack B doesn't carry A's choices over to B.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset is intentionally only keyed on update.pack_key
  useMemo(() => {
    setActions({});
  }, [update?.pack_key]);

  if (!update) return null;

  const setAction = (key: string, action: UpdateAction) => {
    setActions((prev) => ({ ...prev, [key]: action }));
  };

  const bulkTakeUpstream = () => {
    const next: Record<string, UpdateAction> = {};
    for (const d of update.role_diffs) {
      if (d.status !== "unchanged") next[d.role_key] = "take_upstream";
    }
    if (update.variables_differ) next[VARIABLES_ACTION_KEY] = "take_upstream";
    setActions(next);
  };

  const bulkKeepMine = () => {
    const next: Record<string, UpdateAction> = {};
    for (const d of update.role_diffs) {
      if (d.status !== "unchanged") next[d.role_key] = "keep_mine";
    }
    if (update.variables_differ) next[VARIABLES_ACTION_KEY] = "keep_mine";
    setActions(next);
  };

  const onSubmit = async () => {
    try {
      await mutation.mutateAsync({ packKey: update.pack_key, actions });
      toast.success(`Updated ${update.pack_key} to ${update.available_version}`);
      onOpenChange(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "unknown error";
      toast.error(`Could not apply update: ${msg}`);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-h-[90vh] max-w-5xl overflow-y-auto`}>
        <DialogHeader>
          <DialogTitle>
            Update {update.pack_key} · {update.current_version} → {update.available_version}
          </DialogTitle>
          <DialogDescription>
            Review changes from Oxy's upstream pack. Default is to keep your version; pick "Take
            Oxy's" per row to accept upstream changes.
          </DialogDescription>
        </DialogHeader>

        <div className='flex items-center gap-2 border-y py-2'>
          <span className='text-muted-foreground text-xs'>Bulk:</span>
          <Button size='sm' variant='outline' onClick={bulkTakeUpstream}>
            <ArrowDown />
            Take Oxy's for all
          </Button>
          <Button size='sm' variant='outline' onClick={bulkKeepMine}>
            <ArrowUp />
            Keep mine for all
          </Button>
          <span className='ml-auto text-muted-foreground text-xs'>
            Unchanged rows don't need a choice.
          </span>
        </div>

        <div className='flex flex-col gap-3'>
          {update.role_diffs.map((diff) => (
            <RoleDiffRow
              key={diff.role_key}
              diff={diff}
              action={actions[diff.role_key]}
              onChoose={(a) => setAction(diff.role_key, a)}
            />
          ))}

          {update.variables_differ && (
            <VariablesDiffRow
              current={update.current_variables}
              upstream={update.upstream_variables}
              action={actions[VARIABLES_ACTION_KEY]}
              onChoose={(a) => setAction(VARIABLES_ACTION_KEY, a)}
            />
          )}
        </div>

        <DialogFooter>
          <Button
            variant='outline'
            onClick={() => onOpenChange(false)}
            disabled={mutation.isPending}
          >
            Cancel
          </Button>
          <Button onClick={onSubmit} disabled={mutation.isPending}>
            {mutation.isPending ? (
              <>
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                Applying…
              </>
            ) : (
              `Apply → ${update.available_version}`
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default StarterUpdateDialog;

// ── Sub-components ────────────────────────────────────────────────

const RoleDiffRow: React.FC<{
  diff: RoleDiff;
  action: UpdateAction | undefined;
  onChoose: (action: UpdateAction) => void;
}> = ({ diff, action, onChoose }) => {
  const interactive = diff.status !== "unchanged";
  return (
    <div className='flex flex-col gap-2 border border-border p-3'>
      <div className='flex items-center justify-between gap-2'>
        <div className='flex items-center gap-2'>
          <span className='font-medium text-sm'>{diff.role_key}</span>
          <StatusBadge status={diff.status} />
        </div>
        {interactive && (
          <div className='flex items-center gap-1'>
            <Button
              size='sm'
              variant={action === "take_upstream" ? "default" : "outline"}
              onClick={() => onChoose("take_upstream")}
            >
              {action === "take_upstream" && <Check className='h-3 w-3' />}
              Take Oxy's
            </Button>
            <Button
              size='sm'
              variant={action === "keep_mine" ? "default" : "outline"}
              onClick={() => onChoose("keep_mine")}
            >
              {action === "keep_mine" && <Check className='h-3 w-3' />}
              Keep mine
            </Button>
          </div>
        )}
      </div>
      {interactive && (
        <div className='grid gap-2 sm:grid-cols-2'>
          <SideBySidePane label='Yours' content={diff.current?.prompt ?? "(role removed)"} />
          <SideBySidePane label="Oxy's" content={diff.upstream?.prompt ?? "(role removed)"} />
        </div>
      )}
    </div>
  );
};

const VariablesDiffRow: React.FC<{
  current: Record<string, unknown>;
  upstream: Record<string, unknown>;
  action: UpdateAction | undefined;
  onChoose: (action: UpdateAction) => void;
}> = ({ current, upstream, action, onChoose }) => (
  <div className='flex flex-col gap-2 border border-border p-3'>
    <div className='flex items-center justify-between gap-2'>
      <div className='flex items-center gap-2'>
        <span className='font-medium text-sm'>Pack variables</span>
        <Badge variant='secondary'>Differs</Badge>
      </div>
      <div className='flex items-center gap-1'>
        <Button
          size='sm'
          variant={action === "take_upstream" ? "default" : "outline"}
          onClick={() => onChoose("take_upstream")}
        >
          {action === "take_upstream" && <Check className='h-3 w-3' />}
          Take Oxy's
        </Button>
        <Button
          size='sm'
          variant={action === "keep_mine" ? "default" : "outline"}
          onClick={() => onChoose("keep_mine")}
        >
          {action === "keep_mine" && <Check className='h-3 w-3' />}
          Keep mine
        </Button>
      </div>
    </div>
    <div className='grid gap-2 sm:grid-cols-2'>
      <SideBySidePane label='Yours' content={JSON.stringify(current, null, 2)} />
      <SideBySidePane label="Oxy's" content={JSON.stringify(upstream, null, 2)} />
    </div>
  </div>
);

const SideBySidePane: React.FC<{ label: string; content: string }> = ({ label, content }) => (
  <div className='flex flex-col gap-1'>
    <span className='text-muted-foreground text-xs uppercase tracking-wide'>{label}</span>
    <pre className='max-h-64 overflow-auto whitespace-pre-wrap break-words border border-border bg-muted/30 p-2 font-mono text-xs'>
      {content}
    </pre>
  </div>
);

const StatusBadge: React.FC<{ status: RoleDiffStatus }> = ({ status }) => {
  switch (status) {
    case "unchanged":
      return (
        <Badge variant='outline' className='gap-1'>
          <Check className='h-3 w-3' />
          Unchanged
        </Badge>
      );
    case "differs":
      return <Badge variant='secondary'>Differs</Badge>;
    case "only_in_upstream":
      return (
        <Badge variant='outline' className='gap-1'>
          <Plus className='h-3 w-3' />
          Added by Oxy
        </Badge>
      );
    case "only_in_current":
      return (
        <Badge variant='outline' className='gap-1'>
          <Minus className='h-3 w-3' />
          Removed upstream
        </Badge>
      );
  }
};
