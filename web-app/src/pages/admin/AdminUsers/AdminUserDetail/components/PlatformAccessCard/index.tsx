import { ShieldOff } from "lucide-react";
import { useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { ROLE_LABELS, useAppAdmins, useRemoveAppAdmin } from "@/hooks/api/access/useAppAdmins";
import { useDelegationBound } from "@/hooks/api/access/useDelegationBound";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { platformRoleKind, RoleBadge } from "@/pages/admin/components/RoleBadge";
import { GrantDialog } from "./components/GrantDialog";

/** What each role actually buys, in the operator's words rather than the model's. */
const ROLE_BLURB: Record<string, string> = {
  global_admin:
    "Reaches the whole admin console except the Billing queue — including deleting organizations.",
  app_operator: "Ships and develops custom apps. No org deletion, members, billing or settings."
};

/**
 * Staff standing for the person on screen — grant, change, or revoke it here.
 *
 * Platform grants had exactly one home, `/admin/app-admins`, which is a table keyed by
 * email. So the answer to "make this person an App Operator" was: copy their address,
 * navigate away from the record you are looking at, find the form, paste, submit. The
 * fact belongs to the person, and this is where the person is.
 *
 * **Hidden without `manage_platform_grants`.** Not disabled: an operator who cannot
 * administer grants has no use for an inert card, and `useAppAdmins` 403s for them, so
 * rendering it would also mean a failed request behind a control that does nothing.
 * (Hiding is not the control — the server re-decides both writes.)
 *
 * The card reads the same list the grant console does, so the two never disagree about
 * who holds what, and a write from either invalidates the same cache key.
 */
export function PlatformAccessCard({ userEmail }: { userEmail: string }) {
  const bound = useDelegationBound();
  // Gated on the capability: this query is the grant console's, and 403s for anyone else.
  // The whole query, not `data: admins = []`: that default made a failed fetch render
  // "No staff access." — the card asserting the opposite of what it knows, on the one
  // surface an operator checks before handing someone the console.
  const appAdmins = useAppAdmins(bound.canGrant);
  const remove = useRemoveAppAdmin();
  const [granting, setGranting] = useState(false);
  const [confirmRevoke, setConfirmRevoke] = useState(false);

  if (!bound.canGrant) return null;

  const grant = appAdmins.data?.find((a) => a.email.toLowerCase() === userEmail.toLowerCase());
  const kind = platformRoleKind(grant?.role);
  // The delegation bound decides both directions: whether this person's EXISTING grant
  // may be touched, and — when they have none — whether the operator has any role left
  // to issue at all.
  const canWrite = grant ? grant.can_manage : bound.issuableRoles.length > 0;

  return (
    <section
      className='space-y-3 rounded-lg border border-border/60 bg-card p-4'
      data-testid='admin-user-platform-access'
    >
      <div className='flex items-start justify-between gap-3'>
        <div className='min-w-0 space-y-1'>
          <h3 className='font-semibold text-sm'>Staff access</h3>
          {/* `grant` is derived from the same query, so it is only ever read inside the
              success branch. */}
          <AdminAsync query={appAdmins} noun='staff access' rows={1}>
            {() =>
              grant ? (
                <div className='space-y-1'>
                  <div className='flex flex-wrap items-center gap-2'>
                    {kind ? (
                      <RoleBadge kind={kind} />
                    ) : (
                      // A role this build cannot name. The server treats it as
                      // undelegatable rather than guessing, and so does this.
                      <span className='font-mono text-[10px] text-muted-foreground'>
                        {grant.role}
                      </span>
                    )}
                    <span className='text-muted-foreground text-xs'>
                      {grant.scope_all
                        ? "All organizations"
                        : `${grant.scope_org_ids.length} organization${grant.scope_org_ids.length === 1 ? "" : "s"}`}
                    </span>
                  </div>
                  <p className='text-muted-foreground text-xs'>
                    {ROLE_BLURB[grant.role] ?? "Capabilities are derived from the stored role."}
                  </p>
                </div>
              ) : (
                <p className='text-muted-foreground text-xs'>
                  No staff access. This person reaches only the organizations they belong to.
                </p>
              )
            }
          </AdminAsync>
        </div>

        <div
          className='flex shrink-0 items-center gap-1.5'
          title={
            canWrite
              ? undefined
              : grant
                ? `${userEmail} holds a grant at or above your own. Only a Global Owner can change it.`
                : "You have no role left to issue — a grant must be weaker than your own."
          }
        >
          <Button
            variant='outline'
            size='sm'
            disabled={!canWrite}
            onClick={() => setGranting(true)}
            data-testid='admin-user-platform-grant-trigger'
          >
            {grant ? "Change" : "Grant staff access"}
          </Button>
          {grant && (
            <Button
              variant='ghost'
              size='sm'
              disabled={!canWrite}
              className='text-destructive hover:bg-destructive/10 hover:text-destructive'
              onClick={() => setConfirmRevoke(true)}
              data-testid='admin-user-platform-revoke-trigger'
            >
              <ShieldOff className='size-3.5' />
              Revoke
            </Button>
          )}
        </div>
      </div>

      {granting && (
        <GrantDialog
          userEmail={userEmail}
          existing={grant}
          bound={bound}
          onClose={() => setGranting(false)}
        />
      )}

      <AlertDialog
        open={confirmRevoke}
        onOpenChange={(open) => {
          if (!open && !remove.isPending) setConfirmRevoke(false);
        }}
      >
        <AlertDialogContent data-testid='admin-user-platform-revoke-dialog'>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke staff access?</AlertDialogTitle>
            <AlertDialogDescription>
              {userEmail} loses {grant ? (ROLE_LABELS[grant.role] ?? grant.role) : "their grant"}{" "}
              and every organization it reached. Their org memberships are untouched. Recorded to
              the audit log.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={remove.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={remove.isPending}
              onClick={(event) => {
                event.preventDefault();
                if (!grant) return;
                remove.mutate(grant.id, { onSettled: () => setConfirmRevoke(false) });
              }}
            >
              {remove.isPending ? "Revoking…" : "Revoke"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
