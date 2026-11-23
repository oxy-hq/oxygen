import { isAxiosError } from "axios";
import { Loader2, ShieldCheck } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import {
  btnCompact,
  btnCompactPrimary,
  btnCompactSecondary,
  DESIGN_REFERENCE,
  eyebrow,
  note,
  textBase
} from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Separator } from "@/components/ui/shadcn/separator";
import {
  type AccessScope,
  useAppAccess,
  useGrantablePeople,
  useGrantableTeams,
  useSetAppAccess
} from "@/hooks/api/appAccess";
import { cn } from "@/libs/shadcn/utils";
import type { AppVisibility, GranteeRef } from "@/types/appAccess";
import { GrantList } from "./GrantList";
import { GrantPicker } from "./GrantPicker";
import {
  addGrant,
  estimateReach,
  grantedKeys,
  removeGrant,
  setGrantRole,
  toGrantRows
} from "./grants";
import { VisibilityChoice } from "./VisibilityChoice";

interface AppAccessDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scope: AccessScope;
  appId: string | null;
  appName: string;
  /**
   * Draw it in the reference design (the home's, reference-app `HomeAccess.tsx`):
   * square, on hairlines, no icon in the title, capitalised section labels, and
   * Save in the brand fill beside a secondary Cancel. The settings, admin and
   * partner consoles leave it off and keep the dialog as it was.
   */
  reference?: boolean;
}

/** What a failed Save says. 400 is the server rejecting a grantee who isn't an org
 *  member — worth saying plainly, because the picker should have made it impossible. */
function saveErrorMessage(err: unknown): string {
  if (!isAxiosError(err)) return "Couldn't save access";
  if (err.response?.status === 400) {
    return "Someone on this list is no longer a member of the organization. Remove them and try again.";
  }
  if (err.response?.status === 403) {
    return "You don't have permission to change who can open this app.";
  }
  return err.response?.data?.message ?? err.message;
}

/**
 * The dialog's editable copy of the app's access: seeded once per open from a
 * settled response, edited locally, saved in one PUT.
 */
function useAccessForm(open: boolean, scope: AccessScope, appId: string | null) {
  // All three queries take `appId` only while open, so nothing fetches for a dialog
  // nobody has opened — the grant picker's teams/people lists especially, which
  // would otherwise fire on every render of the surface hosting the dialog.
  const activeAppId = open ? appId : null;
  const { data: access, isFetching, isError } = useAppAccess(scope, activeAppId);
  const { data: teams } = useGrantableTeams(scope, activeAppId);
  const { data: people, isError: peopleFailed } = useGrantablePeople(scope, activeAppId);

  const [visibility, setVisibility] = useState<AppVisibility>("org");
  const [grants, setGrants] = useState<GranteeRef[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Seed the editable state ONCE per open, and only from a SETTLED response.
  //
  // The `isFetching` term is load-bearing, not belt-and-braces: without it, a cached
  // entry would seed the form and record the app id, and the fresh response landing
  // a moment later would be skipped by that same guard — quietly re-introducing the
  // stale copy that Save then writes back over someone else's edit. (`gcTime: 0` on
  // the query means there is normally no cache to return, so this is the second lock
  // on the same door.)
  // `useState`, not `useRef`: the render that flips the spinner off has to be
  // *caused* by seeding. With a ref it happened only because `setGrants` passes a
  // freshly-mapped array — a new identity every time, so React never bails out.
  // That made a rendering guarantee load-bearing on something that reads like a
  // formatting detail: memoize that map, or add a deep-equal guard, and the dialog
  // would spin forever with no error. State makes the dependency explicit and costs
  // nothing, since the seeding render already happens.
  const [seededFor, setSeededFor] = useState<string | null>(null);
  useEffect(() => {
    if (!open) {
      if (seededFor !== null) setSeededFor(null);
      return;
    }
    if (!access || isFetching || seededFor === access.app_id) return;
    setSeededFor(access.app_id);
    setVisibility(access.visibility);
    setGrants(access.grants.map((g) => ({ kind: g.kind, id: g.id, role: g.role })));
    setError(null);
  }, [open, access, isFetching, seededFor]);

  // The one readiness test the whole dialog uses: has THIS app's server state been
  // copied into the form yet? Both the spinner and Save key off it, so they can't
  // disagree — a Save enabled before seeding would PUT the defaults (`org`, no
  // grants) and wipe the app's real access list.
  const seeded = !!access && seededFor === access.app_id;

  return {
    access,
    isError,
    seeded,
    teams: teams ?? [],
    people: people ?? [],
    peopleFailed,
    visibility,
    setVisibility,
    grants,
    setGrants,
    error,
    setError
  };
}

type AccessForm = ReturnType<typeof useAccessForm>;

/** The title and the one line under it; the reference design drops the shield. */
function AccessHeader({ appName, reference }: { appName: string; reference: boolean }) {
  return (
    <DialogHeader className={cn("space-y-1 border-b px-6 py-4", reference && "px-5 pt-4 pb-3.5")}>
      <DialogTitle
        className={cn(
          "flex items-center gap-2 text-base",
          reference && cn("font-semibold text-foreground", textBase)
        )}
      >
        {!reference && <ShieldCheck className='size-4 text-muted-foreground' aria-hidden />}
        Who can open {appName}
      </DialogTitle>
      <DialogDescription className={cn(reference && note)}>
        Changes apply immediately after you save. People who lose access stop seeing the app on
        their home page.
      </DialogDescription>
    </DialogHeader>
  );
}

/**
 * The grant list, its picker and the reach under its label. Shown on BOTH
 * branches, not just the restricted one. The server writes the grant list either
 * way — deliberately, because an admin grant on an open app is how a non-officer
 * becomes that app's administrator (`Ring::AppAdmin` has no visibility term). But
 * hiding the list when someone picks "Everyone in the organization" meant the grants
 * silently survived a change the admin reasonably reads as removing them: every
 * member of a team granted `admin` kept the app's privileged surface, with no screen
 * anywhere showing it and no way to remove it without first flipping back to
 * restricted.
 */
function GrantSection({ form, reference }: { form: AccessForm; reference: boolean }) {
  const { grants, setGrants, teams, people, peopleFailed } = form;
  const restricted = form.visibility === "members";
  /** Grant rows joined back to their display data, in the order the server sorts. */
  const rows = useMemo(() => toGrantRows(grants, teams, people), [grants, teams, people]);
  const alreadyGranted = useMemo(() => grantedKeys(grants), [grants]);
  const reachCount = useMemo(
    () => (restricted ? estimateReach(grants, teams) : null),
    [restricted, grants, teams]
  );
  const showReach = restricted && reachCount !== null && grants.length > 0;
  return (
    <section className='flex flex-col gap-3'>
      <div className='flex items-baseline justify-between gap-3'>
        <h3 className={reference ? eyebrow : "font-medium text-sm"}>
          {restricted ? "Access list" : "Roles"}
        </h3>
        {showReach && (
          <p className='text-muted-foreground text-xs'>
            Reaches about {reachCount} {reachCount === 1 ? "person" : "people"}
          </p>
        )}
      </div>

      {!restricted && (
        <p className='text-muted-foreground text-xs leading-relaxed'>
          Everyone in the organization can open this app, so these don't control who gets in.{" "}
          <strong>Can administer</strong> still grants the app's admin surface — remove anyone who
          shouldn't have it. <strong>Can open</strong> applies again only if you restrict the app.
        </p>
      )}

      <GrantPicker
        teams={teams}
        people={people}
        peopleUnavailable={peopleFailed}
        alreadyGranted={alreadyGranted}
        onAdd={(kind, id) => setGrants((prev) => addGrant(prev, kind, id))}
      />

      {/* Say it out loud rather than showing a teams-only picker and letting
          individual grants render as "Unknown person" — which would then be saved
          back under that label. */}
      {peopleFailed && (
        <p className='text-muted-foreground text-xs'>
          Individual people couldn't be loaded, so only teams can be added right now. Any people
          already on the list are shown below and will be kept as they are.
        </p>
      )}

      <GrantList
        rows={rows}
        restricted={restricted}
        onRoleChange={(kind, id, role) => setGrants((prev) => setGrantRole(prev, kind, id, role))}
        onRemove={(kind, id) => setGrants((prev) => removeGrant(prev, kind, id))}
      />
    </section>
  );
}

/**
 * The dialog's body. Spin until this app's state has actually been SEEDED, not
 * merely until the query settles. On the render where `access` first arrives the
 * fetch has already finished, so an `isFetching` test is false while component
 * state is still the defaults — and a restricted app would paint "Everyone in the
 * organization" for one frame before snapping. Gating on the seed subsumes both
 * `isPending` and `isFetching` and says what's meant. `isError` is checked first: on
 * error `access` never arrives, and spinning forever would be worse than the message.
 */
function AccessBody({ form, reference }: { form: AccessForm; reference: boolean }) {
  if (form.isError) {
    return (
      <p className='py-12 text-center text-muted-foreground text-sm'>
        Couldn't load access settings. Close this and try again.
      </p>
    );
  }
  if (!form.seeded) {
    return (
      <div className='flex items-center justify-center py-16'>
        <Loader2 className='size-5 animate-spin text-muted-foreground' aria-hidden />
        <span className='sr-only'>Loading access settings</span>
      </div>
    );
  }
  // Gate the grant section on the SERVER copy, not on `grants` — the in-progress
  // edit state. Reading the edit state made the section unmount the moment you
  // removed the last role, taking `GrantPicker` with it: no undo (the removal is
  // unsaved), and no way back without switching to "Only people you choose" and
  // returning. It also made the forward direction a detour — the very state this
  // section exists for, an open app with a non-officer as its administrator, was
  // uncreatable on an app that had no grants yet. Keyed on the saved copy, the
  // section stays mounted for the life of the edit.
  const hasSavedGrants = (form.access?.grants.length ?? 0) > 0;
  const showGrants = form.visibility === "members" || hasSavedGrants || form.grants.length > 0;
  return (
    <div className='flex flex-col gap-6'>
      <VisibilityChoice
        reference={reference}
        value={form.visibility}
        onChange={form.setVisibility}
      />
      {showGrants && (
        <>
          <Separator />
          <GrantSection form={form} reference={reference} />
        </>
      )}
      {form.error && (
        <p role='alert' className='text-destructive text-sm'>
          {form.error}
        </p>
      )}
    </div>
  );
}

/** Secondary then primary: Save is what the dialog exists for. */
function AccessFooter({
  reference,
  saveDisabled,
  saving,
  onCancel,
  onSave
}: {
  reference: boolean;
  saveDisabled: boolean;
  saving: boolean;
  onCancel: () => void;
  onSave: () => void;
}) {
  return (
    <div
      className={cn(
        "flex items-center justify-end gap-2 border-t px-6 py-4",
        reference && "px-5 py-3"
      )}
    >
      <Button
        variant='ghost'
        onClick={onCancel}
        data-testid='app-access-cancel'
        className={cn(reference && [btnCompact, btnCompactSecondary])}
      >
        Cancel
      </Button>
      <Button
        onClick={onSave}
        disabled={saveDisabled}
        data-testid='app-access-save'
        className={cn(reference && [btnCompact, btnCompactPrimary])}
      >
        {saving && <Loader2 className='size-4 animate-spin' aria-hidden />}
        Save access
      </Button>
    </div>
  );
}

/**
 * Who can open one custom app.
 *
 * One component for all three consoles — the org's own settings, the Oxy admin
 * panel, and the partner console. They differ only in the `scope` they pass, which
 * decides which endpoints the hooks call.
 *
 * Editing is local until Save: `visibility` and the grant list move together in one
 * PUT, so a half-applied change can't leave an app restricted with nobody on it.
 */
export function AppAccessDialog({
  open,
  onOpenChange,
  scope,
  appId,
  appName,
  reference = false
}: AppAccessDialogProps) {
  const form = useAccessForm(open, scope, appId);
  const setAccess = useSetAppAccess(scope);

  const handleSave = async () => {
    if (!appId) return;
    form.setError(null);
    try {
      await setAccess.mutateAsync({
        appId,
        body: { visibility: form.visibility, grants: form.grants }
      });
      toast.success(
        form.visibility === "org"
          ? `Everyone in the organization can open ${appName}`
          : `Access to ${appName} updated`
      );
      onOpenChange(false);
    } catch (err) {
      form.setError(saveErrorMessage(err));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        // Portalled out of the page that opened it, so it carries the reference scope itself.
        className={cn(
          "flex max-h-[85vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl",
          reference && [DESIGN_REFERENCE, "border-border-subtle bg-card shadow-xl"]
        )}
        data-reference={reference || undefined}
      >
        <AccessHeader appName={appName} reference={reference} />
        <div className='flex-1 overflow-y-auto px-6 py-5'>
          <AccessBody form={form} reference={reference} />
        </div>
        <AccessFooter
          reference={reference}
          saveDisabled={!form.seeded || form.isError || setAccess.isPending}
          saving={setAccess.isPending}
          onCancel={() => onOpenChange(false)}
          onSave={handleSave}
        />
      </DialogContent>
    </Dialog>
  );
}
