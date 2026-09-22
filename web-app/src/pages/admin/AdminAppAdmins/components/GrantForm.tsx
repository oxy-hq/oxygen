import { Loader2, RefreshCw, UserPlus } from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { ROLE_LABELS, useAppAdmins, useCreateAppAdmin } from "@/hooks/api/access/useAppAdmins";
import type { DelegationBound } from "@/hooks/api/access/useDelegationBound";
import { useDrainedAdminOrgs } from "@/hooks/api/adminTenants";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { AppAdmin, PlatformRoleId } from "@/types/access";

/**
 * What each role buys, in the operator's words. Kept short on purpose — the
 * authoritative expansion is `PlatformRole::caps()` in `oxy-authz`, and the granted
 * row's real capability list is rendered from the server's response, not from here.
 */
const ROLE_BLURB: Record<PlatformRoleId, string> = {
  app_operator: "Ships and develops custom apps. No orgs, members, billing, or infrastructure.",
  global_admin: "The whole admin console except Billing queue and this page."
};

/**
 * Issue or replace a platform grant: who, which role, and how far it reaches.
 *
 * `editing` is a grant the table asked to load — the edit affordance the table itself
 * lacks. Without it the only way to change someone's role is to retype their address
 * into a form captioned "Grant access", which is how a replace reads as an add.
 *
 * **Mount-time initialisation, not an effect.** The parent remounts this component (via
 * a `key`) when Edit is pressed, so the row is read once on the way in. An effect keyed
 * on `[editing]` looked equivalent and wasn't: pressing Edit on the same row twice hands
 * React the identical object reference, so the state update bails, the deps never
 * change, and the effect never refires — the form silently keeps whatever was typed
 * locally, which is the opposite of what re-opening a row should mean.
 *
 * `onCancel` is **required**, not optional. It is not just the way out of edit mode: with
 * the field-by-field clearing gone, the parent's remount IS the reset, so a caller that
 * omitted it would leave the form holding the grant it just wrote — email included, which
 * makes `replacing` true and puts a destructive red "Replace grant" on the button aimed
 * at the row that was already saved. The type says what the prose below says: this
 * component does not reset itself.
 */
export function GrantForm({
  editing,
  onCancel,
  bound
}: {
  editing?: AppAdmin;
  onCancel: () => void;
  /**
   * What this operator may issue. Passed in rather than read here so the page and the
   * form cannot disagree about it — the table disables rows from the same source.
   */
  bound: DelegationBound;
}) {
  const create = useCreateAppAdmin();
  const { data: admins = [] } = useAppAdmins();
  const [email, setEmail] = useState(editing?.email ?? "");
  // App Operator is the default because it is the least privileged of the two. The API
  // defaults to Global Admin for compatibility with clients that send only an email;
  // a human standing in front of a picker should start at the narrow end.
  // Least privilege first, and never a role this operator cannot issue: defaulting to
  // `app_operator` unconditionally would be right for a Global Admin and wrong for a
  // future tier that cannot issue even that — `issuableRoles` is the authority.
  const [role, setRole] = useState<PlatformRoleId>(
    editing?.role ?? bound.issuableRoles[0] ?? "app_operator"
  );
  const [bounded, setBounded] = useState(editing ? !editing.scope_all : false);
  const [orgIds, setOrgIds] = useState<string[]>(editing?.scope_org_ids ?? []);

  // Only fetched once the operator actually chooses to bound the grant — most grants
  // are unbounded, and this page shouldn't pull the org directory to render a form.
  // Drained: a picker capped at 50 silently cannot assign the 51st org.
  const {
    orgs: allOrgs,
    isLoading,
    isDraining,
    isIncomplete,
    error,
    refetch
  } = useDrainedAdminOrgs({ enabled: bounded });
  // A bounded operator may only put orgs from their OWN scope inside a grant — the
  // server refuses anything wider (`Scope::contains`). Offering the full directory here
  // would be offering a 403, and would also leak which tenants exist to someone whose
  // grant deliberately does not reach them.
  const orgs = useMemo(
    () => (bound.scopeAll ? allOrgs : allOrgs.filter((o) => bound.scopeOrgIds.includes(o.id))),
    [allOrgs, bound.scopeAll, bound.scopeOrgIds]
  );
  // See AssignToOrgDialog: a scope picker that reports "No organizations found" mid-drain
  // is telling an operator the org they want does not exist.
  const orgsLoading = isLoading || isDraining;

  // The endpoint UPSERTS: submitting an email that already holds a grant replaces its
  // role and scope outright. A form that says "Grant access" and shows nothing else
  // therefore hides a destructive edit — an owner could type a Global Admin's address,
  // leave the defaults, and silently downgrade them. The list is already in hand, so
  // say so before the fact rather than reporting it in a toast afterwards.
  const trimmed = email.trim().toLowerCase();
  const replacing = trimmed ? admins.find((a) => a.email.toLowerCase() === trimmed) : undefined;

  const submit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!trimmed) return;
    create.mutate(
      { email: trimmed, role, ...(bounded ? { scope_org_ids: orgIds } : {}) },
      {
        // Leaving edit mode remounts the form (the parent bumps its nonce), which IS
        // the reset — so there is no field-by-field clearing here to fall out of sync.
        // The hand-rolled version reset email, orgIds and bounded but not `role`, so
        // after saving a Global Admin the next grant started pre-selected at the BROAD
        // role, quietly contradicting the least-privilege default three lines above.
        //
        // Safe on this path: the mutation-level `onSuccess` in `useCreateAppAdmin` fires
        // the toast and the cache invalidation before this callback runs, so unmounting
        // here loses nothing.
        onSuccess: () => onCancel()
      }
    );
  };

  const toggleOrg = (id: string) =>
    setOrgIds((prev) => (prev.includes(id) ? prev.filter((o) => o !== id) : [...prev, id]));

  return (
    <Card className='mb-6' data-testid='admin-app-admins-grant-form'>
      <CardContent className='p-4'>
        <form onSubmit={submit} className='flex flex-col gap-4'>
          <div className='flex flex-col gap-3 sm:flex-row sm:items-start'>
            <div className='flex-1'>
              <Label htmlFor='grant-email' className='text-xs'>
                Email
              </Label>
              <Input
                id='grant-email'
                type='email'
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder='operator@example.com'
                disabled={create.isPending}
                autoComplete='off'
                className='mt-1'
                data-testid='admin-app-admins-email-input'
              />
            </div>
            <div className='sm:w-56'>
              <Label htmlFor='grant-role' className='text-xs'>
                Role
              </Label>
              <Select value={role} onValueChange={(v) => setRole(v as PlatformRoleId)}>
                <SelectTrigger
                  id='grant-role'
                  className='mt-1'
                  data-testid='admin-app-admins-role-select'
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {/* Only what this operator may issue. A Global Admin offered "Global
                      Admin" here would be offered a 403 — the delegation bound admits a
                      grant strictly weaker than their own, never an equal one. Rendering
                      the option disabled was the alternative; a role you can never pick,
                      on a form whose whole job is picking one, is worse than its absence
                      once the note below says why. */}
                  {bound.issuableRoles.map((r) => (
                    <SelectItem key={r} value={r}>
                      {ROLE_LABELS[r]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          <p className='text-muted-foreground text-xs'>{ROLE_BLURB[role]}</p>

          {/* The absence above, explained in the place it bites. Owners see nothing —
              they can issue every role, so there is no gap to account for. */}
          {!bound.issuableRoles.includes("global_admin") && (
            <p className='text-muted-foreground text-xs' data-testid='admin-app-admins-role-note'>
              Only a Global Owner can grant Global Admin.
            </p>
          )}

          {replacing && (
            <div
              className='rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs'
              data-testid='admin-app-admins-replace-warning'
            >
              <span className='font-medium'>{replacing.email}</span> already holds{" "}
              <span className='font-medium'>{ROLE_LABELS[replacing.role] ?? replacing.role}</span>{" "}
              over{" "}
              {replacing.scope_all
                ? "all organizations"
                : `${replacing.scope_org_ids.length} organization${replacing.scope_org_ids.length === 1 ? "" : "s"}`}
              . Saving replaces it.
            </div>
          )}

          <div className='flex flex-col gap-2 border-border border-t pt-3'>
            <div className='flex items-center gap-2'>
              <Checkbox
                id='grant-bounded'
                checked={bounded}
                onCheckedChange={(v) => setBounded(v === true)}
                data-testid='admin-app-admins-scope-toggle'
              />
              <Label htmlFor='grant-bounded' className='font-normal text-xs'>
                Limit to specific organizations
              </Label>
            </div>

            {bounded && (
              <div
                className='max-h-48 overflow-auto rounded-md border border-border p-2'
                data-testid='admin-app-admins-scope-orgs'
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
                    // `Checkbox` renders a button, not an input, so the label has to be
                    // associated by id — wrapping it doesn't count.
                    <Label
                      key={org.id}
                      htmlFor={`grant-org-${org.id}`}
                      className='flex cursor-pointer items-center gap-2 rounded px-1 py-1 font-normal hover:bg-muted/40'
                    >
                      <Checkbox
                        id={`grant-org-${org.id}`}
                        checked={orgIds.includes(org.id)}
                        onCheckedChange={() => toggleOrg(org.id)}
                      />
                      <span className='text-xs'>{org.name}</span>
                      <span className='font-mono text-[10px] text-muted-foreground'>
                        {org.slug}
                      </span>
                    </Label>
                  ))
                )}
              </div>
            )}

            {/* An empty bounded scope is a valid grant that reaches nothing. Saying so
                beats silently creating a powerless row the operator will later file a
                bug about. */}
            {bounded && orgIds.length === 0 && !orgsLoading && (
              <p className='text-muted-foreground text-xs'>
                No organizations selected — this grant will reach none.
              </p>
            )}
          </div>

          <div className='flex items-center justify-between'>
            <span className='text-muted-foreground text-xs'>
              {bounded ? (
                <Badge variant='outline'>
                  {orgIds.length} organization{orgIds.length === 1 ? "" : "s"}
                </Badge>
              ) : (
                <Badge variant='outline'>All organizations</Badge>
              )}
            </span>
            <div className='flex items-center gap-2'>
              {editing && (
                <Button
                  type='button'
                  variant='ghost'
                  onClick={onCancel}
                  disabled={create.isPending}
                  data-testid='admin-app-admins-cancel-edit'
                >
                  Cancel
                </Button>
              )}
              <Button
                type='submit'
                variant={replacing ? "destructive" : "default"}
                disabled={create.isPending || trimmed.length === 0}
                data-testid='admin-app-admins-submit'
              >
                {create.isPending ? (
                  <>
                    <Loader2 className='size-4 animate-spin' />
                    Saving…
                  </>
                ) : replacing ? (
                  // Name the actual outcome. "Grant access" on a call that overwrites an
                  // existing role is the label lying about the verb.
                  <>
                    <RefreshCw className='size-4' />
                    Replace grant
                  </>
                ) : (
                  <>
                    <UserPlus className='size-4' />
                    Grant access
                  </>
                )}
              </Button>
            </div>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
