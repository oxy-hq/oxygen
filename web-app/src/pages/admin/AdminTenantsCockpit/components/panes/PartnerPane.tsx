import { ShieldAlert, Trash2 } from "lucide-react";
import { useState } from "react";
import { AssumeRoleDialog } from "@/components/admin/AssumeRoleDialog";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogDestructiveAction,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger
} from "@/components/ui/shadcn/alert-dialog";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  useAdminPartnerDetail,
  useAttachPartnerOrg,
  useDetachPartnerOrg,
  useRevokePartnership
} from "@/hooks/api/adminPartners";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import OrgPicker from "../OrgPicker";
import PartnerCeiling from "../PartnerCeiling";
import PartnerOrgChart from "../PartnerOrgChart";
import PartnerPeopleList from "../PartnerPeopleList";
import { PaneHeader, PaneLoading, PaneSection, RowLine, Stat } from "../paneParts";

/**
 * A partner, from Oxy's side of the table.
 *
 * A partner IS an org — `partnerId` here is an org id, and the name/slug are the
 * org's own. Oxy governs exactly two things from this pane: the **ceiling** (what
 * this partner may ever do) and **which clients** it manages. Partner access
 * (who is an operator) is normally the partner's own to manage, but staff can
 * grant/revoke it here too. **Act as** enters the partner's console.
 */
export default function PartnerPane({
  partnerId,
  onClose
}: {
  /** The partner's org id. */
  partnerId: string;
  onClose: () => void;
}) {
  const partnerQuery = useAdminPartnerDetail(partnerId);
  const revoke = useRevokePartnership();
  const attachOrg = useAttachPartnerOrg(partnerId);
  const detachOrg = useDetachPartnerOrg(partnerId);
  const [assumeOpen, setAssumeOpen] = useState(false);

  return (
    // The pane's own three-bar skeleton is kept — the kit's equal bars would
    // misstate a header-over-sections layout.
    <AdminAsync query={partnerQuery} noun='the partner' className='p-4' skeleton={<PaneLoading />}>
      {(partner) => {
        const suspended = partner.status !== "active";
        return (
          <div className='pb-10'>
            <PaneHeader
              eyebrow='Partner'
              title={partner.name}
              subtitle={`${partner.slug} · partner since ${new Date(partner.created_at).toLocaleDateString()}`}
              status={
                suspended ? (
                  <Badge variant='destructive'>suspended</Badge>
                ) : (
                  <Badge variant='secondary'>active</Badge>
                )
              }
              actions={
                <>
                  {/* Enter the partner's console. A partner IS an org, so this is the same
                assume-role session as any tenant — landing on /partners because the
                session is flagged is_partner. */}
                  <Button
                    size='sm'
                    variant='outline'
                    className={cn("border-warning/40", ADMIN_TONE.warn.text)}
                    onClick={() => setAssumeOpen(true)}
                  >
                    <ShieldAlert className='mr-1.5 size-4' />
                    Act as
                  </Button>
                  <AlertDialog>
                    <AlertDialogTrigger asChild>
                      <Button variant='ghost' size='sm' className='text-destructive'>
                        <Trash2 className='size-4' />
                      </Button>
                    </AlertDialogTrigger>
                    <AlertDialogContent>
                      <AlertDialogHeader>
                        <AlertDialogTitle>
                          Revoke {partner.name}&apos;s partnership?
                        </AlertDialogTitle>
                        <AlertDialogDescription>
                          It stops managing its {partner.managed_orgs.length} client
                          {partner.managed_orgs.length === 1 ? "" : "s"} and its people lose partner
                          access. <b>{partner.name} itself survives</b> — it is a real organization
                          with its own workspaces and data, and keeps all of them.
                        </AlertDialogDescription>
                      </AlertDialogHeader>
                      <AlertDialogFooter>
                        <AlertDialogCancel>Cancel</AlertDialogCancel>
                        <AlertDialogDestructiveAction
                          onClick={() => revoke.mutate(partner.org_id, { onSuccess: onClose })}
                        >
                          Revoke
                        </AlertDialogDestructiveAction>
                      </AlertDialogFooter>
                    </AlertDialogContent>
                  </AlertDialog>
                </>
              }
            />

            <div className='space-y-5 p-4'>
              <div className='grid grid-cols-3 gap-2'>
                <Stat label='Clients' value={partner.managed_orgs.length} />
                <Stat label='Operators' value={partner.people.filter((p) => p.has_access).length} />
                <Stat label='Status' value={partner.status} />
              </div>

              <PaneSection title='Ceiling'>
                <PartnerCeiling orgId={partner.org_id} capabilities={partner.capabilities} />
              </PaneSection>

              <PaneSection
                title='Clients'
                action={
                  <OrgPicker
                    label='Attach client…'
                    // Never itself, and never an org someone else already manages.
                    exclude={[partner.org_id, ...partner.managed_orgs.map((o) => o.org_id)]}
                    onPick={(o) => attachOrg.mutate(o.id)}
                  />
                }
              >
                {partner.managed_orgs.length === 0 ? (
                  <p className='text-muted-foreground text-xs'>No clients yet.</p>
                ) : (
                  <div className='space-y-2'>
                    {partner.managed_orgs.map((o) => (
                      <RowLine
                        key={o.org_id}
                        primary={o.org_name ?? o.org_id}
                        secondary={o.org_slug ?? undefined}
                        trailing={
                          <Button
                            variant='ghost'
                            size='sm'
                            onClick={() => detachOrg.mutate(o.org_id)}
                          >
                            Detach
                          </Button>
                        }
                      />
                    ))}
                  </div>
                )}
              </PaneSection>

              <PaneSection title='People'>
                <p className='-mt-1 text-muted-foreground text-xs'>
                  Toggle partner access per person. An operator reaches every client, within the
                  ceiling above. Normally {partner.name}&apos;s own owner/admin manages this; staff
                  changes are audited.
                </p>
                <PartnerPeopleList partner={partner} />
              </PaneSection>

              <PaneSection title='Hierarchy'>
                <PartnerOrgChart partner={partner} />
              </PaneSection>
            </div>

            <AssumeRoleDialog
              open={assumeOpen}
              onOpenChange={setAssumeOpen}
              org={{ id: partner.org_id, name: partner.name }}
            />
          </div>
        );
      }}
    </AdminAsync>
  );
}
