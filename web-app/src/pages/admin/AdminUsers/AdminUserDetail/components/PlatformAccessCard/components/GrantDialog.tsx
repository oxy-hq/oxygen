import { Loader2, ShieldCheck } from "lucide-react";
import { useMemo, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { ROLE_LABELS, useCreateAppAdmin } from "@/hooks/api/access/useAppAdmins";
import type { DelegationBound } from "@/hooks/api/access/useDelegationBound";
import { useDrainedAdminOrgs } from "@/hooks/api/adminTenants";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { AppAdmin, PlatformRoleId } from "@/types/access";

const ROLE_BLURB: Record<PlatformRoleId, string> = {
  global_admin:
    "Reaches the whole admin console except the Billing queue — including deleting organizations.",
  app_operator: "Ships and develops custom apps. No org deletion, members, billing or settings."
};

/**
 * Issue or change one person's platform grant.
 *
 * The email is fixed — it is the record being viewed — which is the whole reason this
 * exists beside the grant console's form rather than reusing it. That form's first field
 * is an address to type, and typing an address that already holds a grant silently
 * replaces it; here there is nothing to mistype and the existing grant is stated above
 * the controls.
 *
 * Both pickers are bounded by what the operator may issue, mirroring
 * `oxy_authz::may_delegate`. That is presentation, not enforcement: the server refuses a
 * role at or above the caller's own and a scope wider than theirs regardless.
 */
export function GrantDialog({
  userEmail,
  existing,
  bound,
  onClose
}: {
  userEmail: string;
  /** The grant being replaced, when there is one. */
  existing?: AppAdmin;
  bound: DelegationBound;
  onClose: () => void;
}) {
  const create = useCreateAppAdmin();
  const [role, setRole] = useState<PlatformRoleId>(
    existing?.role ?? bound.issuableRoles[0] ?? "app_operator"
  );
  const [bounded, setBounded] = useState(existing ? !existing.scope_all : false);
  const [orgIds, setOrgIds] = useState<string[]>(existing?.scope_org_ids ?? []);

  // Only once the operator chooses to bound the grant — most are unbounded, and opening
  // this dialog should not pull the org directory. Drained: a picker capped at 50
  // silently cannot reach the 51st org.
  const {
    orgs: allOrgs,
    isLoading,
    isDraining,
    isIncomplete,
    error,
    refetch
  } = useDrainedAdminOrgs({ enabled: bounded });
  const orgsLoading = isLoading || isDraining;
  // A bounded operator can only bound a grant to orgs inside their own reach; the server
  // refuses anything wider, so offering the rest would be offering a 403.
  const orgs = useMemo(
    () => (bound.scopeAll ? allOrgs : allOrgs.filter((o) => bound.scopeOrgIds.includes(o.id))),
    [allOrgs, bound.scopeAll, bound.scopeOrgIds]
  );

  const submit = () =>
    create.mutate(
      { email: userEmail, role, ...(bounded ? { scope_org_ids: orgIds } : {}) },
      { onSuccess: onClose }
    );

  return (
    <Dialog open onOpenChange={(open) => !open && !create.isPending && onClose()}>
      <DialogContent className='max-w-lg' data-testid='admin-user-platform-grant-dialog'>
        <DialogHeader>
          <DialogTitle className='text-sm'>
            {existing ? "Change staff access" : "Grant staff access"}
          </DialogTitle>
          <DialogDescription className='text-xs'>
            Gives <span className='font-medium text-foreground'>{userEmail}</span> reach across
            organizations they are not a member of. Recorded to the audit log.
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-3'>
          <div>
            <Label htmlFor='platform-grant-role' className='text-xs'>
              Role
            </Label>
            <Select value={role} onValueChange={(v) => setRole(v as PlatformRoleId)}>
              <SelectTrigger
                id='platform-grant-role'
                className='mt-1'
                data-testid='admin-user-platform-role-select'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {bound.issuableRoles.map((r) => (
                  <SelectItem key={r} value={r}>
                    {ROLE_LABELS[r]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className='mt-1 text-muted-foreground text-xs'>{ROLE_BLURB[role]}</p>
          </div>

          {/* Replacing is the destructive case and the one an operator can walk into
              without noticing — say what is there now, before the click. */}
          {existing && (
            <div
              className='rounded-md border border-border bg-muted/40 p-2 text-xs'
              data-testid='admin-user-platform-replacing'
            >
              Currently{" "}
              <span className='font-medium'>{ROLE_LABELS[existing.role] ?? existing.role}</span>{" "}
              over{" "}
              {existing.scope_all
                ? "all organizations"
                : `${existing.scope_org_ids.length} organization${existing.scope_org_ids.length === 1 ? "" : "s"}`}
              . Saving replaces it.
            </div>
          )}

          <div className='flex flex-col gap-2 border-border border-t pt-3'>
            <div className='flex items-center gap-2'>
              <Checkbox
                id='platform-grant-bounded'
                checked={bounded}
                onCheckedChange={(v) => setBounded(v === true)}
                data-testid='admin-user-platform-scope-toggle'
              />
              <Label htmlFor='platform-grant-bounded' className='font-normal text-xs'>
                Limit to specific organizations
              </Label>
            </div>

            {bounded && (
              <div
                className='max-h-48 overflow-auto rounded-md border border-border p-2'
                data-testid='admin-user-platform-scope-orgs'
              >
                {/* A failed drain is a THIRD outcome, not an empty directory. `isDraining` goes
                    false when the drain stops, so without the `isIncomplete` branch below a page-2
                    failure renders the "no organizations" copy — telling an operator the org they
                    want does not exist. Same rule and same renderer as the rest of the admin panel:
                    the server's own message, and a Retry. */}
                {orgsLoading ? (
                  <p className='p-2 text-muted-foreground text-xs'>Loading organizations…</p>
                ) : isIncomplete ? (
                  <AdminAsync
                    className='m-2'
                    query={{ isError: true, data: undefined, error, refetch }}
                    noun='organizations'
                  >
                    {() => null}
                  </AdminAsync>
                ) : orgs.length === 0 ? (
                  <p className='p-2 text-muted-foreground text-xs'>
                    {bound.scopeAll
                      ? "No organizations found."
                      : "Your own grant reaches no organizations, so there is nothing to bound this one to."}
                  </p>
                ) : (
                  orgs.map((org) => (
                    // `Checkbox` renders a button, not an input, so the label associates
                    // by id — wrapping it does not count.
                    <Label
                      key={org.id}
                      htmlFor={`platform-grant-org-${org.id}`}
                      className='flex cursor-pointer items-center gap-2 rounded px-1 py-1 font-normal hover:bg-muted/40'
                    >
                      <Checkbox
                        id={`platform-grant-org-${org.id}`}
                        checked={orgIds.includes(org.id)}
                        onCheckedChange={() =>
                          setOrgIds((prev) =>
                            prev.includes(org.id)
                              ? prev.filter((o) => o !== org.id)
                              : [...prev, org.id]
                          )
                        }
                      />
                      <span className='truncate text-xs'>{org.name}</span>
                      <span className='truncate font-mono text-[10px] text-muted-foreground'>
                        {org.slug}
                      </span>
                    </Label>
                  ))
                )}
              </div>
            )}

            {/* A bounded grant with nothing selected is valid and reaches nothing.
                Saying so beats a validation error for a state that is legal. */}
            {bounded && orgIds.length === 0 && !orgsLoading && (
              <p className='text-muted-foreground text-xs'>
                No organizations selected — this grant will reach none.
              </p>
            )}
          </div>
        </div>

        <DialogFooter>
          <Button variant='ghost' onClick={onClose} disabled={create.isPending}>
            Cancel
          </Button>
          <Button
            onClick={submit}
            disabled={create.isPending || bound.issuableRoles.length === 0}
            data-testid='admin-user-platform-grant-submit'
          >
            {create.isPending ? (
              <>
                <Loader2 className='size-3.5 animate-spin' />
                Saving…
              </>
            ) : (
              <>
                <ShieldCheck className='size-3.5' />
                {existing ? "Replace grant" : "Grant access"}
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
