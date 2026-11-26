import { Pencil, ShieldAlert } from "lucide-react";
import { useMemo, useState } from "react";
import { toast } from "sonner";
import { AssumeRoleDialog } from "@/components/admin/AssumeRoleDialog";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import {
  usePartnerHealth,
  usePartnerOrgMembers,
  usePartnerOrgWorkspaces,
  useUpdateClientOrg
} from "@/hooks/api/partners";
import { cn } from "@/libs/shadcn/utils";
import {
  PaneHeader,
  PaneSection,
  Stat
} from "@/pages/admin/AdminTenantsCockpit/components/paneParts";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "@/pages/admin/components/AdminTable";
import type { PartnerCapabilities } from "@/types/auth";
import type { ChildOrg, PartnerHealthRow } from "@/types/partners";
import InviteMemberForm from "../../components/InviteMemberForm";
import MemberRow from "../../components/MemberRow";
import OrgAppsPanel from "../../components/OrgAppsPanel";

/**
 * One client org, from the partner's side — the partner-scoped mirror of the
 * admin org detail. Same capabilities the partner's ceiling allows (members,
 * apps, rename, act-as), same admin look, every action on the surface.
 */
export default function ClientPane({
  partnerId,
  org,
  caps
}: {
  partnerId: string;
  org: ChildOrg;
  caps: PartnerCapabilities;
}) {
  const [assumeOpen, setAssumeOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState(false);

  return (
    <div className='pb-10'>
      <PaneHeader
        eyebrow='Client'
        title={org.name}
        subtitle={org.slug}
        actions={
          <>
            {caps.manage_org_settings && (
              <Button
                variant='outline'
                size='sm'
                onClick={() => setRenameOpen(true)}
                data-testid='partner-client-rename'
              >
                <Pencil className='mr-1.5 size-3.5' />
                Rename
              </Button>
            )}
            {caps.develop_apps && (
              <Button
                size='sm'
                variant='outline'
                className='border-status-warning/40 text-status-warning'
                onClick={() => setAssumeOpen(true)}
                data-testid='partner-client-act-as'
              >
                <ShieldAlert className='mr-1.5 size-4' />
                Act as
              </Button>
            )}
          </>
        }
      />

      <div className='space-y-5 p-4'>
        <div className='grid grid-cols-2 gap-2'>
          <Stat label='Members' value={org.member_count} />
          <Stat label='Apps' value={org.app_count} />
        </div>

        {caps.manage_members && (
          <PaneSection
            title='Members'
            action={<InviteMemberForm partnerId={partnerId} orgId={org.org_id} />}
          >
            <MembersTable partnerId={partnerId} orgId={org.org_id} />
          </PaneSection>
        )}

        {caps.manage_apps && (
          <PaneSection title='Apps'>
            <div className='overflow-hidden border border-border-subtle'>
              <OrgAppsPanel partnerId={partnerId} orgId={org.org_id} />
            </div>
          </PaneSection>
        )}

        {caps.manage_apps && (
          <PaneSection title='Workspaces'>
            <WorkspacesList partnerId={partnerId} orgId={org.org_id} />
          </PaneSection>
        )}
      </div>

      <AssumeRoleDialog
        open={assumeOpen}
        onOpenChange={setAssumeOpen}
        org={{ id: org.org_id, name: org.name }}
      />
      <RenameDialog
        open={renameOpen}
        onOpenChange={setRenameOpen}
        partnerId={partnerId}
        orgId={org.org_id}
        name={org.name}
      />
    </div>
  );
}

function MembersTable({ partnerId, orgId }: { partnerId: string; orgId: string }) {
  const { data: members, isLoading } = usePartnerOrgMembers(partnerId, orgId);

  if (isLoading) return <Skeleton className='h-32 w-full' />;
  if (!members?.length)
    return <p className='text-muted-foreground text-sm'>This organization has no members.</p>;

  return (
    <Table>
      <TableHeader>
        <TableRow className={ADMIN_HEADER_ROW_CLASS}>
          <AdminTh>Email</AdminTh>
          <AdminTh>Name</AdminTh>
          <AdminTh>Role</AdminTh>
          <AdminTh align='right'>Actions</AdminTh>
        </TableRow>
      </TableHeader>
      <TableBody>
        {members.map((m) => (
          <MemberRow key={m.user_id} partnerId={partnerId} orgId={orgId} member={m} />
        ))}
      </TableBody>
    </Table>
  );
}

function WorkspacesList({ partnerId, orgId }: { partnerId: string; orgId: string }) {
  const { data: workspaces, isLoading } = usePartnerOrgWorkspaces(partnerId, orgId);
  // Health is a partner-wide rollup; index it by workspace so each row can show
  // its own signal. This is the old standalone "Workspace health" page, folded in.
  const { data: health } = usePartnerHealth(partnerId);
  const healthById = useMemo(
    () => new Map((health ?? []).map((h) => [h.workspace_id, h])),
    [health]
  );

  if (isLoading) return <Skeleton className='h-24 w-full' />;
  if (!workspaces?.length)
    return <p className='text-muted-foreground text-sm'>No workspaces in this organization.</p>;

  return (
    <Table>
      <TableHeader>
        <TableRow className={ADMIN_HEADER_ROW_CLASS}>
          <AdminTh>Workspace</AdminTh>
          <AdminTh>Status</AdminTh>
          <AdminTh>Health</AdminTh>
          <AdminTh align='right'>Last opened</AdminTh>
        </TableRow>
      </TableHeader>
      <TableBody>
        {workspaces.map((w) => (
          <TableRow key={w.id} className='border-border-subtle'>
            <TableCell className='font-medium'>
              {w.name}
              {!w.has_revision && (
                <span className='ml-2 text-muted-foreground text-xs'>· not compiled</span>
              )}
            </TableCell>
            <TableCell>
              <Badge
                variant='outline'
                className={cn(
                  "px-1.5 py-0 text-[10px] capitalize",
                  w.status === "error"
                    ? "border-destructive/40 text-destructive"
                    : w.status === "ready"
                      ? "border-border text-muted-foreground"
                      : "border-status-warning/40 text-status-warning"
                )}
              >
                {w.status}
              </Badge>
            </TableCell>
            <TableCell>
              <WorkspaceHealth row={healthById.get(w.id)} />
            </TableCell>
            <TableCell className='text-right text-muted-foreground text-xs tabular-nums'>
              {w.last_opened_at ? new Date(w.last_opened_at).toLocaleDateString() : "—"}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/** One workspace's health signal (from the partner health sweep), with reasons on hover. */
function WorkspaceHealth({ row }: { row?: PartnerHealthRow }) {
  if (!row || row.status === "unknown")
    return <span className='text-muted-foreground text-xs'>—</span>;

  const badge = (
    <Badge
      variant='outline'
      className={cn(
        "px-1.5 py-0 text-[10px] capitalize",
        row.status === "unhealthy"
          ? "border-destructive/40 text-destructive"
          : row.status === "degraded"
            ? "border-status-warning/40 text-status-warning"
            : "border-border text-muted-foreground"
      )}
    >
      {row.status}
    </Badge>
  );

  if (!row.reasons.length) return badge;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span>{badge}</span>
      </TooltipTrigger>
      <TooltipContent className={`${DESIGN_REFERENCE} max-w-xs`}>
        <ul className='space-y-0.5 text-xs'>
          {row.reasons.map((r) => (
            <li key={r}>{r}</li>
          ))}
        </ul>
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Rename a client. The **slug** is deliberately not editable: it's the org's
 * public identity (subdomains, custom-app URLs), so changing it breaks live links
 * — the client's own call, not their partner's.
 */
function RenameDialog({
  open,
  onOpenChange,
  partnerId,
  orgId,
  name
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  partnerId: string;
  orgId: string;
  name: string;
}) {
  const [value, setValue] = useState(name);
  const update = useUpdateClientOrg(partnerId);

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (o) setValue(name);
        onOpenChange(o);
      }}
    >
      <DialogContent className={`${DESIGN_REFERENCE} max-w-sm`}>
        <DialogHeader>
          <DialogTitle>Rename client</DialogTitle>
        </DialogHeader>
        <Input
          value={value}
          autoFocus
          onChange={(e) => setValue(e.target.value)}
          data-testid='partner-client-rename-name'
        />
        <p className='text-muted-foreground text-xs'>The slug stays fixed — it backs live URLs.</p>
        <DialogFooter>
          <Button
            variant='ghost'
            onClick={() => onOpenChange(false)}
            data-testid='partner-client-rename-cancel'
          >
            Cancel
          </Button>
          <Button
            data-testid='partner-client-rename-save'
            disabled={!value.trim() || update.isPending}
            onClick={() =>
              update.mutate(
                { orgId, name: value.trim() },
                {
                  onSuccess: () => {
                    toast.success("Client renamed");
                    onOpenChange(false);
                  },
                  onError: () => toast.error("Failed to rename")
                }
              )
            }
          >
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
