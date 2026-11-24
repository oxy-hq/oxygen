import { Copy, Loader2, RotateCw, Search, Trash2, UserPlus, Users } from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import {
  useCreateInvitation,
  useOrgInvitations,
  useOrgMembers,
  useRevokeInvitation
} from "@/hooks/api/organizations";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import type { Organization, OrgRole } from "@/types/organization";
import TableWrapper from "../../../../components/TableWrapper";
import { SectionActions } from "../../../components/SectionChrome";

import { InviteDialog } from "./components/InviteDialog";
import { MemberRowActions } from "./components/MemberRowActions";
import { RoleBadge } from "./components/RoleBadge";

interface TeamSectionProps {
  org: Organization;
  viewerRole: OrgRole;
}

export default function TeamSection({ org, viewerRole }: TeamSectionProps) {
  const orgId = org.id;
  const { data: currentUser } = useCurrentUser();
  const { data: members, isPending, isError } = useOrgMembers(orgId);
  const revokeInvitation = useRevokeInvitation();
  const createInvitation = useCreateInvitation();
  const [search, setSearch] = useState("");
  const [inviteOpen, setInviteOpen] = useState(false);

  const canManage = viewerRole === "owner" || viewerRole === "admin";
  // `GET /orgs/:id/invitations` is OrgAdmin-guarded while the roster itself is
  // readable by any member, so firing it unconditionally made every member who
  // opened this section trip the global 403 toast — a denial notice for a
  // request the page never needed. Only ask when the answer is renderable.
  const { data: invitations } = useOrgInvitations(orgId, canManage);
  const ownerCount = members?.filter((m) => m.role === "owner").length ?? 0;
  const adminCount = members?.filter((m) => m.role === "admin").length ?? 0;
  // Expired invites are included deliberately — they're the ones that need
  // clearing, and they used to be filtered out server-side, leaving an admin
  // blocked by a row no screen would show them.
  const pendingInvitations = invitations?.filter((inv) => inv.status === "pending") ?? [];
  const expiredCount = pendingInvitations.filter((inv) => inv.is_expired).length;

  const filtered = useMemo(() => {
    const q = search.toLowerCase().trim();
    if (!q || !members) return members ?? [];
    return members.filter(
      (m) => m.email.toLowerCase().includes(q) || (m.name || "").toLowerCase().includes(q)
    );
  }, [members, search]);

  const handleRevoke = async (invitationId: string) => {
    try {
      await revokeInvitation.mutateAsync({ orgId, invitationId });
      toast.success("Invitation revoked");
    } catch {
      toast.error("Failed to revoke invitation");
    }
  };

  /**
   * Re-send a lapsed invitation. No dedicated endpoint needed: creating an
   * invite supersedes the expired row for the same address, so this issues a
   * fresh token and email and clears the stale row in one call.
   */
  const handleResend = async (email: string, role: OrgRole) => {
    try {
      await createInvitation.mutateAsync({ orgId, email, role });
      toast.success(`Invitation resent to ${email}`);
    } catch {
      toast.error("Failed to resend invitation");
    }
  };

  if (isPending) {
    return (
      <div className='flex min-h-40 w-full items-center justify-center'>
        <Loader2 className='h-4 w-4 animate-spin text-muted-foreground' />
      </div>
    );
  }

  if (isError) {
    return (
      <div className='flex min-h-40 w-full items-center justify-center'>
        <p className='text-destructive text-sm'>Failed to load members.</p>
      </div>
    );
  }

  const memberCount = members?.length ?? 0;
  const counts = `${memberCount} ${memberCount === 1 ? "member" : "members"} · ${adminCount} ${adminCount === 1 ? "admin" : "admins"}`;

  return (
    <div className='flex flex-col gap-5'>
      {canManage && (
        <SectionActions>
          <Button onClick={() => setInviteOpen(true)} size='sm' className='gap-1.5'>
            <UserPlus className='size-4' aria-hidden='true' />
            Invite member
          </Button>
        </SectionActions>
      )}

      <p className='text-muted-foreground text-sm'>{counts}</p>

      <div className='relative'>
        <Search className='absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2 text-muted-foreground' />
        <Input
          placeholder='Search members...'
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className='pl-9'
        />
      </div>

      {filtered.length === 0 ? (
        <div className='flex flex-col items-center gap-3 rounded-md border py-12 text-center'>
          <Users className='h-8 w-8 text-muted-foreground/30' />
          <p className='text-muted-foreground text-sm'>
            {search ? `No members match "${search}"` : "No members yet"}
          </p>
          {!search && canManage && (
            <Button
              size='sm'
              variant='outline'
              className='mt-1 gap-1.5'
              onClick={() => setInviteOpen(true)}
            >
              <UserPlus className='h-4 w-4' />
              Invite your first member
            </Button>
          )}
        </div>
      ) : (
        <TableWrapper>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className='px-4'>Name</TableHead>
                <TableHead className='w-32 px-4'>Role</TableHead>
                <TableHead className='w-12' />
              </TableRow>
            </TableHeader>
            <TableBody>
              {filtered.map((member) => {
                const isSelf = member.user_id === currentUser?.id;
                const displayName = member.name || member.email.split("@")[0];

                return (
                  <TableRow key={member.id}>
                    <TableCell data-label='Member' className='px-4 py-3 max-md:px-0 max-md:py-0'>
                      <div className='flex items-center gap-3'>
                        <div className='flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-muted font-medium text-sm'>
                          {member.name?.[0]?.toUpperCase() ?? "?"}
                        </div>
                        <div className='flex flex-col gap-0.5'>
                          <span className='font-medium text-sm leading-none'>
                            {displayName}
                            {isSelf && (
                              <span className='ml-1.5 text-muted-foreground text-xs'>(you)</span>
                            )}
                          </span>
                          <span className='font-mono text-muted-foreground text-xs'>
                            {member.email}
                          </span>
                        </div>
                      </div>
                    </TableCell>
                    <TableCell
                      data-label='Role'
                      className='w-32 px-4 py-3 max-md:w-auto max-md:px-0 max-md:py-0'
                    >
                      <RoleBadge role={member.role} />
                    </TableCell>
                    <TableCell className='w-12 px-2 py-3 text-right max-md:w-auto max-md:px-0 max-md:py-0'>
                      <MemberRowActions
                        member={member}
                        orgId={orgId}
                        viewerRole={viewerRole}
                        isSelf={isSelf}
                        ownerCount={ownerCount}
                      />
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </TableWrapper>
      )}

      {canManage && pendingInvitations.length > 0 && (
        <div className='space-y-3'>
          <h3 className='font-medium'>
            Pending Invitations
            {expiredCount > 0 ? (
              <span className='ml-2 font-normal text-amber-600 text-xs dark:text-amber-500'>
                {expiredCount} expired
              </span>
            ) : null}
          </h3>
          <div className='divide-y divide-border rounded-lg border border-border'>
            {pendingInvitations.map((inv) => (
              <div key={inv.id} className='flex items-center gap-3 px-4 py-3'>
                <div className='min-w-0 flex-1'>
                  <div className='truncate text-sm'>{inv.email}</div>
                  <div className='text-muted-foreground text-xs capitalize'>{inv.role}</div>
                </div>
                {inv.is_expired ? (
                  <div className='text-amber-600 text-xs dark:text-amber-500'>
                    Expired {new Date(inv.expires_at).toLocaleDateString()}
                  </div>
                ) : (
                  <div className='text-muted-foreground text-xs'>
                    Expires {new Date(inv.expires_at).toLocaleDateString()}
                  </div>
                )}
                {/* An expired token can't be accepted, so copying its link would
                    hand out a dead URL. Offer a fresh invite instead. */}
                {inv.is_expired ? (
                  <Button
                    variant='ghost'
                    size='icon'
                    disabled={createInvitation.isPending}
                    onClick={() => handleResend(inv.email, inv.role)}
                    title='Send a new invitation'
                  >
                    <RotateCw className='h-4 w-4 text-muted-foreground' />
                  </Button>
                ) : (
                  <Button
                    variant='ghost'
                    size='icon'
                    onClick={async () => {
                      const inviteUrl = `${window.location.origin}/invite/${inv.token}`;
                      try {
                        await navigator.clipboard.writeText(inviteUrl);
                        toast.success("Invite link copied");
                      } catch {
                        toast.error("Failed to copy invite link");
                      }
                    }}
                    title='Copy invite link'
                  >
                    <Copy className='h-4 w-4 text-muted-foreground' />
                  </Button>
                )}
                <Button
                  variant='ghost'
                  size='icon'
                  onClick={() => handleRevoke(inv.id)}
                  title='Revoke invitation'
                >
                  <Trash2 className='h-4 w-4 text-muted-foreground' />
                </Button>
              </div>
            ))}
          </div>
        </div>
      )}

      <InviteDialog
        open={inviteOpen}
        onOpenChange={setInviteOpen}
        orgId={orgId}
        viewerRole={viewerRole}
      />
    </div>
  );
}
