import { Building2, Check, Loader2, Search, UserPlus } from "lucide-react";
import { useMemo, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useAddUserToOrg, useDrainedAdminOrgs } from "@/hooks/api/adminTenants";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { OrgRoleId } from "@/services/api/adminTenants";

/** What each role actually grants, in the operator's words rather than the model's. */
const ROLE_BLURB: Record<OrgRoleId, string> = {
  owner: "Full control, including billing, deleting the org, and transferring ownership.",
  admin: "Manages members, workspaces and settings. No billing, no deleting the org.",
  member: "Ordinary access to the org's workspaces and apps."
};

/**
 * Put a person into an organization, at a role.
 *
 * The endpoint has existed for a while and had **no UI at all** — assigning someone to a
 * tenant meant calling the API by hand. It is also one of the most consequential writes
 * staff can make: it grants standing inside a customer's org, and `owner` there can
 * delete that org outright. So this dialog does three things a bare form wouldn't:
 *
 * 1. names what the chosen role grants, before the click, not in a toast after it;
 * 2. shows the orgs the person is ALREADY in, disabled — the server answers 409 for a
 *    duplicate, and a disabled row explains that better than an error does;
 * 3. calls out `owner` specifically, because it is the one choice here that hands over
 *    the ability to destroy the tenant.
 *
 * Every assignment made through this is recorded to `audit_events` by the handler,
 * inside the same transaction as the membership — see `admin_membership_audit`.
 */
export function AssignToOrgDialog({
  userId,
  userEmail,
  existingOrgIds
}: {
  userId: string;
  userEmail: string;
  /** Orgs this person already belongs to — offered but disabled, not hidden. */
  existingOrgIds: string[];
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [orgId, setOrgId] = useState<string | null>(null);
  const [role, setRole] = useState<OrgRoleId>("member");
  const add = useAddUserToOrg();

  // Only fetched once the dialog opens — this list is the whole tenant directory.
  // Drained: a picker capped at 50 silently cannot assign the 51st org.
  const { orgs, isLoading, isDraining, isIncomplete, error, refetch } = useDrainedAdminOrgs({
    enabled: open
  });
  // A picker must not say "no match" while pages are still arriving — an operator reads
  // that as "not there" and stops. `isLoading` alone was right for the rail (where the
  // alternative was a skeleton before you could type) and wrong here.
  const isPending = isLoading || isDraining;

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    if (!needle) return orgs;
    return orgs.filter(
      (o) => o.name.toLowerCase().includes(needle) || o.slug.toLowerCase().includes(needle)
    );
  }, [orgs, search]);

  const selected = orgs.find((o) => o.id === orgId);
  const alreadyIn = (id: string) => existingOrgIds.includes(id);

  const reset = () => {
    setSearch("");
    setOrgId(null);
    setRole("member");
  };

  const submit = () => {
    if (!orgId) return;
    add.mutate(
      { userId, orgId, role },
      {
        onSuccess: () => {
          setOpen(false);
          reset();
        }
      }
    );
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) reset();
      }}
    >
      <DialogTrigger asChild>
        <Button variant='outline' size='sm' data-testid='admin-user-assign-org-trigger'>
          <UserPlus className='size-3.5' />
          Assign to org
        </Button>
      </DialogTrigger>

      <DialogContent className='max-w-lg' data-testid='admin-user-assign-org-dialog'>
        <DialogHeader>
          <DialogTitle className='text-sm'>Assign to organization</DialogTitle>
          <DialogDescription className='text-xs'>
            Grants <span className='font-medium text-foreground'>{userEmail}</span> standing inside
            a tenant. Recorded to the audit log.
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-3'>
          <div>
            <Label htmlFor='assign-org-search' className='text-xs'>
              Organization
            </Label>
            <div className='relative mt-1'>
              <Search className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
              <Input
                id='assign-org-search'
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder='Search by name or slug'
                className='pl-7'
                data-testid='admin-user-assign-org-search'
              />
            </div>
          </div>

          <div
            className='max-h-56 overflow-auto rounded-md border border-border'
            data-testid='admin-user-assign-org-list'
          >
            {/* A failed drain is a THIRD outcome, not an empty directory. `isDraining` goes
                false when the drain stops, so without the `isIncomplete` branch below a page-2
                failure renders the "no organizations" copy — telling an operator the org they
                want does not exist. Same rule and same renderer as the rest of the admin panel:
                the server's own message, and a Retry. */}
            {isPending ? (
              <p className='p-3 text-muted-foreground text-xs'>Loading organizations…</p>
            ) : isIncomplete ? (
              <AdminAsync
                className='m-2'
                query={{ isError: true, data: undefined, error, refetch }}
                noun='organizations'
              >
                {() => null}
              </AdminAsync>
            ) : filtered.length === 0 ? (
              <p className='p-3 text-muted-foreground text-xs'>No organizations match.</p>
            ) : (
              filtered.map((org) => {
                const member = alreadyIn(org.id);
                return (
                  <button
                    key={org.id}
                    type='button'
                    disabled={member}
                    onClick={() => setOrgId(org.id)}
                    className={cn(
                      "flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs transition-colors",
                      member
                        ? "cursor-not-allowed text-muted-foreground"
                        : "cursor-pointer hover:bg-muted/50",
                      orgId === org.id && "bg-muted"
                    )}
                  >
                    <Building2 className='size-3.5 shrink-0 text-muted-foreground' />
                    <span className='truncate font-medium'>{org.name}</span>
                    <span className='truncate font-mono text-[10px] text-muted-foreground'>
                      {org.slug}
                    </span>
                    {/* Disabled + labelled beats hiding: "why isn't Acme in the list"
                        is a worse question than seeing Acme marked already-a-member. */}
                    {member && (
                      <span className='ml-auto shrink-0 text-[10px]'>Already a member</span>
                    )}
                    {orgId === org.id && !member && (
                      <Check className='ml-auto size-3.5 shrink-0 text-primary' />
                    )}
                  </button>
                );
              })
            )}
          </div>

          <div>
            <Label htmlFor='assign-org-role' className='text-xs'>
              Role
            </Label>
            <Select value={role} onValueChange={(v) => setRole(v as OrgRoleId)}>
              <SelectTrigger
                id='assign-org-role'
                className='mt-1'
                data-testid='admin-user-assign-org-role'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {/* Least privilege first — the default and the top of the list agree. */}
                <SelectItem value='member'>Member</SelectItem>
                <SelectItem value='admin'>Admin</SelectItem>
                <SelectItem value='owner'>Owner</SelectItem>
              </SelectContent>
            </Select>
            <p className='mt-1 text-muted-foreground text-xs'>{ROLE_BLURB[role]}</p>
          </div>

          {/* Owner is the one option here that hands over the ability to destroy the
              tenant. Say so before the click rather than in a toast after it. */}
          {role === "owner" && (
            <div
              className='rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs'
              data-testid='admin-user-assign-org-owner-warning'
            >
              Owner can delete {selected ? selected.name : "this organization"} and transfer
              ownership. Assign it only when the person is meant to run the tenant.
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant='ghost' onClick={() => setOpen(false)} disabled={add.isPending}>
            Cancel
          </Button>
          <Button
            onClick={submit}
            disabled={!orgId || add.isPending}
            variant={role === "owner" ? "destructive" : "default"}
            data-testid='admin-user-assign-org-submit'
          >
            {add.isPending ? (
              <>
                <Loader2 className='size-3.5 animate-spin' />
                Assigning…
              </>
            ) : (
              <>
                <UserPlus className='size-3.5' />
                {selected ? `Add to ${selected.name}` : "Add to organization"}
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
