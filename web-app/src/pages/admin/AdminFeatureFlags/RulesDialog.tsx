import { isAxiosError } from "axios";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { useAdminOrgOptions, useUpdateFeatureFlag } from "@/hooks/api/featureFlags";
import type { FeatureFlag } from "@/services/api/featureFlags";
import { FallthroughEditor } from "./FallthroughEditor";
import { OverrideList } from "./OverrideList";
import { buildTargeting, draftFromFlag, isSplitValid } from "./rules";

function serverMessage(err: unknown): string {
  if (isAxiosError(err)) return err.response?.data?.error ?? err.message;
  return err instanceof Error ? err.message : "Failed to save rules.";
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className='space-y-2.5'>
      <h3 className='font-semibold text-sm'>{title}</h3>
      {children}
    </section>
  );
}

/** Edit who gets which variant of one per-org flag. Mount it per edit so the draft starts fresh. */
export function RulesDialog({ flag, onClose }: { flag: FeatureFlag; onClose: () => void }) {
  const update = useUpdateFeatureFlag();
  const { orgs } = useAdminOrgOptions();
  const [draft, setDraft] = useState(() => draftFromFlag(flag));
  const [error, setError] = useState<string | null>(null);

  const save = (targeting: ReturnType<typeof buildTargeting> | null, done: string) => {
    setError(null);
    update.mutate(
      { key: flag.key, targeting },
      {
        onSuccess: () => {
          toast.success(done);
          onClose();
        },
        onError: (err) => setError(serverMessage(err))
      }
    );
  };

  const canSave = !update.isPending && (draft.mode === "variant" || isSplitValid(draft.percents));

  return (
    <Dialog open onOpenChange={(open) => !open && !update.isPending && onClose()}>
      <DialogContent
        className='max-h-[85vh] gap-5 overflow-y-auto sm:max-w-lg'
        data-testid='feature-flag-rules-dialog'
      >
        <DialogHeader>
          <DialogTitle>
            Rules for <span className='font-mono'>{flag.key}</span>
          </DialogTitle>
          <DialogDescription className='text-xs'>
            Orgs and people listed here get their variant; everyone else gets the rule under
            Everyone else. The flag's On switch still turns it all off.
          </DialogDescription>
        </DialogHeader>

        {flag.targeting_error && (
          <p className='text-destructive text-xs' data-testid='feature-flag-rules-stored-error'>
            Stored rules can't be served: {flag.targeting_error}. Save to replace them.
          </p>
        )}

        <Section title='Everyone else'>
          <FallthroughEditor variants={flag.variants} draft={draft} onChange={setDraft} />
        </Section>
        <Section title='Orgs'>
          <OverrideList
            kind='org'
            entries={draft.orgs}
            variants={flag.variants}
            orgs={orgs}
            onChange={(next) => setDraft({ ...draft, orgs: next })}
          />
        </Section>
        <Section title='Users'>
          <OverrideList
            kind='user'
            entries={draft.users}
            variants={flag.variants}
            orgs={orgs}
            onChange={(next) => setDraft({ ...draft, users: next })}
          />
        </Section>

        {error && (
          <p
            className='text-destructive text-xs'
            role='alert'
            data-testid='feature-flag-rules-error'
          >
            {error}
          </p>
        )}

        <DialogFooter className='sm:justify-between'>
          <Button
            type='button'
            variant='ghost'
            size='sm'
            className='text-destructive text-xs hover:text-destructive'
            disabled={update.isPending || (!flag.targeting && !flag.targeting_error)}
            onClick={() => save(null, `Cleared rules for ${flag.key}.`)}
            data-testid='feature-flag-rules-clear'
          >
            Clear rules
          </Button>
          <div className='flex gap-2'>
            <Button
              type='button'
              variant='outline'
              size='sm'
              className='text-xs'
              disabled={update.isPending}
              onClick={onClose}
              data-testid='feature-flag-rules-cancel'
            >
              Cancel
            </Button>
            <Button
              type='button'
              size='sm'
              className='text-xs'
              disabled={!canSave}
              onClick={() => save(buildTargeting(flag, draft), `Saved rules for ${flag.key}.`)}
              data-testid='feature-flag-rules-save'
            >
              {update.isPending ? "Saving…" : "Save"}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
