import {
  Activity,
  Building2,
  DollarSign,
  FolderOpen,
  Handshake,
  Loader2,
  Pencil,
  ShieldAlert,
  Trash2,
  Users
} from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { AssumeRoleDialog } from "@/components/admin/AssumeRoleDialog";
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
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useAdminOrgUsage } from "@/hooks/api/adminMetrics/useAdminOrgUsage";
import {
  useAdminOrgDetail,
  useDeleteAdminOrg,
  useRenameAdminOrg
} from "@/hooks/api/adminTenants/useAdminOrgs";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import GrantPartnershipDialog from "@/pages/admin/AdminTenantsCockpit/components/GrantPartnershipDialog";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import { OltpTenantPanel } from "@/pages/admin/components/OltpTenantPanel";
import { OrgLogoEditor } from "@/pages/admin/components/OrgLogoEditor";
import { AdminAsync } from "../../components/AdminAsync";
import { AdminDetailEyebrow, AdminDetailHeader } from "../../components/AdminDetailHeader";
import { AdminDetailStats } from "../../components/AdminDetailStats";
import { AdminEmptyState } from "../../components/AdminEmptyState";
import { AdminLinkedList, AdminLinkedRow } from "../../components/AdminLinkedRow";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { AdminStatusPill } from "../../components/AdminStatusPill";
import { ADMIN_TONE } from "../../components/adminTone";
import { OrgActivityTab } from "./components/OrgActivityTab";
import { OrgBillingTab } from "./components/OrgBillingTab";
import { OrgCompilesTab } from "./components/OrgCompilesTab";
import { OrgOverviewTab } from "./components/OrgOverviewTab";
import { RoleBadge, WorkspaceStatusPill } from "./components/StatusBadges";
import { compactInt, usd } from "./format";
import { OrgSubdomainSettings } from "./OrgSubdomainSettings";
import type { OrgTabId } from "./tabs";

const USAGE_DAYS = 30;
const ageDays = (iso: string) =>
  Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 86_400_000));

/**
 * `/admin/orgs/:orgId` — the **Tenant 360**: the operator's single surface for
 * investigating one organization. Spine of the unified tenants console — the
 * org page IS the org's user list, workspace list, activity feed, cost
 * snapshot, and (for owners) billing, all cross-linked. The active tab is
 * mirrored to `?tab=` so the command palette and the overview's "needs
 * attention" feed can deep-link straight to the right section.
 */
export default function AdminOrgDetail({
  orgId: orgIdProp,
  embedded = false
}: {
  orgId?: string;
  embedded?: boolean;
} = {}) {
  const { orgId: orgIdParam = "" } = useParams<{ orgId: string }>();
  const orgId = orgIdProp ?? orgIdParam;
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [assumeOpen, setAssumeOpen] = useState(false);
  const [grantOpen, setGrantOpen] = useState(false);

  const org = useAdminOrgDetail(orgId);
  const detail = org.data;
  const { data: currentUser } = useCurrentUser();
  const usage = useAdminOrgUsage(orgId, USAGE_DAYS);
  const rename = useRenameAdminOrg();
  const remove = useDeleteAdminOrg();

  const isOwner = currentUser?.is_owner ?? false;

  // De-tabbed: every section is on one scrolling page, so an operator sees (and
  // acts on) everything without a single tab click. The overview's triage alerts
  // and quick-links scroll to the relevant section instead of switching a tab. A
  // `?section=` param deep-links + scrolls (palette, attention feed).
  const scrollToSection = (id: OrgTabId) => {
    document
      .getElementById(`org-section-${id}`)
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
    setSearchParams(
      (prev) => {
        prev.set("section", id);
        return prev;
      },
      { replace: true }
    );
  };

  const deepLinkSection = searchParams.get("section");
  useEffect(() => {
    if (!deepLinkSection) return;
    const t = setTimeout(() => {
      document
        .getElementById(`org-section-${deepLinkSection}`)
        ?.scrollIntoView({ behavior: "smooth", block: "start" });
    }, 50);
    return () => clearTimeout(t);
  }, [deepLinkSection]);

  useEffect(() => {
    if (detail) {
      setName(detail.name);
      setSlug(detail.slug);
    }
  }, [detail]);

  // Nothing below renders without the org, so the gate is the whole page. It used to be
  // `isLoading || !detail` around a spinner, so a *failed* fetch spun forever — an
  // outage and a slow network looked identical, with no way back but a page reload.
  // `AdminAsync` tells them apart and offers a Retry; the guard covers every
  // non-success state, so its render callback is unreachable.
  if (!detail) {
    return (
      <div className={embedded ? "p-4" : "mx-auto max-w-7xl p-6 lg:px-10 lg:py-10"}>
        <AdminAsync query={org} noun='this organization' rows={4}>
          {() => null}
        </AdminAsync>
      </div>
    );
  }

  const ownerCount = detail.owners.filter((m) => m.role === "owner").length;
  const adminCount = detail.owners.filter((m) => m.role === "admin").length;
  const readyWorkspaces = detail.workspaces.filter((w) => w.status === "ready").length;
  // Compile rows carry only workspace_id; map to names for the Compiles tab.
  const workspaceNames = Object.fromEntries(detail.workspaces.map((w) => [w.id, w.name]));
  // The managing partner (if this is a client org) — powers the "Managed by" jump.
  const partner = detail.partner;

  const handleSave = () => {
    rename.mutate(
      {
        orgId: detail.id,
        body: {
          name: name !== detail.name ? name : undefined,
          slug: slug !== detail.slug ? slug : undefined
        }
      },
      { onSuccess: () => setEditing(false) }
    );
  };

  const handleDelete = () => {
    remove.mutate(detail.id, {
      onSuccess: () => {
        setConfirmDelete(false);
        navigate(ROUTES.ADMIN.ORGS);
      }
    });
  };

  return (
    <div
      className={embedded ? "space-y-6 p-4" : "mx-auto max-w-7xl space-y-6 p-6 lg:px-10 lg:py-10"}
    >
      <AdminDetailHeader
        eyebrow={
          embedded ? undefined : (
            <AdminDetailEyebrow
              segments={[
                { label: "Admin", to: ROUTES.ADMIN.TENANTS },
                { label: "Tenants", to: ROUTES.ADMIN.TENANTS },
                { label: "Organizations", to: ROUTES.ADMIN.ORGS },
                { label: detail.name }
              ]}
            />
          )
        }
        icon={Building2}
        title={detail.name}
        subtitle={
          <>
            <span className='font-mono text-xs'>/{detail.slug}</span>
            <span aria-hidden>·</span>
            <CopyableId value={detail.id} />
            <span aria-hidden>·</span>
            <span>Owner {detail.owner_email ?? "—"}</span>
            <span aria-hidden>·</span>
            <span>{ageDays(detail.created_at).toLocaleString()}d old</span>
          </>
        }
        status={
          <AdminStatusPill
            tone={detail.member_count > 0 ? "ok" : "muted"}
            label={detail.member_count > 0 ? "Active" : "Empty"}
          />
        }
        actions={
          <>
            {/* Entering a tenant is deliberate: without a live session an operator
                has no synthetic Owner membership and 403s like anyone else. */}
            <Button
              variant='outline'
              size='sm'
              className={cn("border-warning/40", ADMIN_TONE.warn.text)}
              onClick={() => setAssumeOpen(true)}
            >
              <ShieldAlert className='size-3.5' />
              Act as
            </Button>
            {/* On a managed org, jump to the managing partner; on an unmanaged one,
                offer to grant a partnership (the only partner-creation entry point). */}
            {partner ? (
              <Button
                variant='outline'
                size='sm'
                onClick={() => navigate(`${ROUTES.ADMIN.TENANTS}?type=partners&id=${partner.id}`)}
              >
                <Handshake className='size-3.5' />
                Managed by {partner.name}
              </Button>
            ) : (
              <Button variant='outline' size='sm' onClick={() => setGrantOpen(true)}>
                <Handshake className='size-3.5' />
                Grant partnership
              </Button>
            )}
            <Button variant='outline' size='sm' onClick={() => setEditing(true)}>
              <Pencil className='size-3.5' />
              Edit
            </Button>
            <Button
              variant='outline'
              size='sm'
              className='text-destructive hover:bg-destructive/10 hover:text-destructive'
              onClick={() => setConfirmDelete(true)}
            >
              <Trash2 className='size-3.5' />
              Delete
            </Button>
          </>
        }
      />

      <AdminDetailStats
        items={[
          {
            label: "Members",
            value: detail.member_count.toLocaleString(),
            sub: `${ownerCount} owner${ownerCount === 1 ? "" : "s"} · ${adminCount} admin${adminCount === 1 ? "" : "s"}`,
            icon: Users
          },
          {
            label: "Workspaces",
            value: detail.workspace_count.toLocaleString(),
            sub: detail.workspaces.length > 0 ? `${readyWorkspaces} ready` : "—",
            icon: FolderOpen
          },
          {
            label: `LLM cost · ${USAGE_DAYS}d`,
            value: usage.isLoading ? "—" : usd(usage.data?.total.cost_usd ?? 0),
            sub: usage.isLoading
              ? "loading…"
              : usage.data && usage.data.total.run_count > 0
                ? `${compactInt(usage.data.total.run_count)} runs`
                : "no activity",
            icon: DollarSign
          },
          {
            label: `Runs · ${USAGE_DAYS}d`,
            value: usage.isLoading ? "—" : compactInt(usage.data?.total.run_count ?? 0),
            sub: "agent runs",
            icon: Activity
          }
        ]}
      />

      <OrgOverviewTab
        detail={detail}
        usage={usage.data}
        usageLoading={usage.isLoading}
        usageDays={USAGE_DAYS}
        onSelectTab={scrollToSection}
      />

      <section id='org-section-members' className='scroll-mt-4 space-y-3'>
        <AdminSectionLabel
          trailing={
            <span className='tabular-nums'>{detail.owners.length.toLocaleString()} total</span>
          }
        >
          Members
        </AdminSectionLabel>
        {detail.owners.length === 0 ? (
          <AdminEmptyState icon={Users} title='No members yet' />
        ) : (
          <AdminLinkedList>
            {detail.owners.map((m) => (
              <AdminLinkedRow
                key={m.user_id}
                to={ROUTES.ADMIN.USER_DETAIL(m.user_id)}
                icon={Users}
                primary={m.name || m.email}
                secondary={m.email}
                meta={<RoleBadge role={m.role} />}
              />
            ))}
          </AdminLinkedList>
        )}
      </section>

      <section id='org-section-workspaces' className='scroll-mt-4 space-y-3'>
        <AdminSectionLabel
          trailing={
            <span className='tabular-nums'>{detail.workspaces.length.toLocaleString()} total</span>
          }
        >
          Workspaces
        </AdminSectionLabel>
        {detail.workspaces.length === 0 ? (
          <AdminEmptyState icon={FolderOpen} title='No workspaces yet' />
        ) : (
          <AdminLinkedList>
            {detail.workspaces.map((w) => (
              <AdminLinkedRow
                key={w.id}
                to={ROUTES.ADMIN.WORKSPACE_DETAIL(w.id)}
                icon={FolderOpen}
                primary={w.name}
                secondary={`Created ${new Date(w.created_at).toLocaleDateString()}`}
                meta={<WorkspaceStatusPill status={w.status} />}
              />
            ))}
          </AdminLinkedList>
        )}
      </section>

      <section id='org-section-activity' className='scroll-mt-4 space-y-3'>
        <AdminSectionLabel>Activity</AdminSectionLabel>
        <OrgActivityTab orgId={detail.id} />
      </section>

      <section id='org-section-compiles' className='scroll-mt-4 space-y-3'>
        <AdminSectionLabel>Compiles</AdminSectionLabel>
        <OrgCompilesTab orgId={detail.id} workspaceNames={workspaceNames} />
      </section>

      <section id='org-section-oltp' className='scroll-mt-4 space-y-3'>
        <AdminSectionLabel>OLTP database</AdminSectionLabel>
        <OltpTenantPanel orgId={detail.id} />
      </section>

      {isOwner ? (
        <section id='org-section-billing' className='scroll-mt-4 space-y-3'>
          <AdminSectionLabel>Billing</AdminSectionLabel>
          <OrgBillingTab orgId={detail.id} />
        </section>
      ) : null}

      <section id='org-section-settings' className='scroll-mt-4 space-y-4'>
        <AdminSectionLabel>Settings</AdminSectionLabel>
        <section className='space-y-4 rounded-lg border border-border/60 bg-card p-6'>
          <div className='space-y-1'>
            <h3 className='font-semibold text-sm'>Branding</h3>
            <p className='text-muted-foreground text-xs'>
              The org logo white-labels the workspace HQ chrome for every member.
            </p>
          </div>
          <OrgLogoEditor orgId={detail.id} name={detail.name} />
        </section>

        <section className='space-y-4 rounded-lg border border-border/60 bg-card p-6'>
          <div className='space-y-1'>
            <h3 className='font-semibold text-sm'>Identity</h3>
            <p className='text-muted-foreground text-xs'>
              The slug appears in every workspace URL for this organization. Changing it rewrites
              every link.
            </p>
          </div>
          <div className='grid gap-4 sm:grid-cols-2'>
            <div className='space-y-1.5'>
              <Label htmlFor='org-name'>Name</Label>
              <Input
                id='org-name'
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder='Acme Corp'
              />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor='org-slug'>Slug</Label>
              <Input
                id='org-slug'
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                placeholder='acme'
                className='font-mono'
              />
            </div>
          </div>
          <div className='flex justify-end gap-2 border-border/60 border-t pt-4'>
            <Button
              variant='ghost'
              size='sm'
              onClick={() => {
                setName(detail.name);
                setSlug(detail.slug);
              }}
              disabled={rename.isPending}
            >
              Reset
            </Button>
            <Button
              size='sm'
              onClick={handleSave}
              disabled={rename.isPending || (name === detail.name && slug === detail.slug)}
            >
              {rename.isPending ? <Loader2 className='size-3 animate-spin' /> : null}
              Save changes
            </Button>
          </div>
        </section>

        <OrgSubdomainSettings orgId={detail.id} />

        <section className='space-y-4 rounded-lg border border-destructive/40 bg-destructive/5 p-6'>
          <div className='space-y-1'>
            <h3 className='font-semibold text-destructive text-sm'>Danger zone</h3>
            <p className='text-destructive/80 text-xs'>
              Removing this organization deletes every workspace, member, billing record, and
              history. This cannot be undone.
            </p>
          </div>
          <Button
            variant='outline'
            size='sm'
            className='border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive'
            onClick={() => setConfirmDelete(true)}
          >
            <Trash2 className='size-3.5' />
            Delete organization
          </Button>
        </section>
      </section>

      {/* Edit dialog — kept simple; opens from the header Edit button too. */}
      <AlertDialog
        open={editing}
        onOpenChange={(o) => !o && !rename.isPending && setEditing(false)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Edit organization</AlertDialogTitle>
            <AlertDialogDescription>
              Update the public-facing name and URL slug.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className='space-y-3'>
            <div className='space-y-1.5'>
              <Label htmlFor='edit-org-name'>Name</Label>
              <Input id='edit-org-name' value={name} onChange={(e) => setName(e.target.value)} />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor='edit-org-slug'>Slug</Label>
              <Input
                id='edit-org-slug'
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                className='font-mono'
              />
            </div>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={rename.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={rename.isPending}
              onClick={(e) => {
                e.preventDefault();
                handleSave();
              }}
            >
              {rename.isPending ? "Saving…" : "Save"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={confirmDelete}
        onOpenChange={(o) => !o && !remove.isPending && setConfirmDelete(false)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete organization?</AlertDialogTitle>
            <AlertDialogDescription>
              <strong>{detail.name}</strong> and all of its workspaces, members, and billing history
              will be removed. This action cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={remove.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={remove.isPending}
              onClick={(e) => {
                e.preventDefault();
                handleDelete();
              }}
              className='bg-destructive text-destructive-foreground hover:bg-destructive/90'
            >
              {remove.isPending ? "Deleting…" : "Delete"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AssumeRoleDialog
        open={assumeOpen}
        onOpenChange={setAssumeOpen}
        org={{ id: detail.id, name: detail.name }}
      />

      <GrantPartnershipDialog
        open={grantOpen}
        onOpenChange={setGrantOpen}
        org={{ id: detail.id, name: detail.name }}
        suggestedAdminEmail={detail.owner_email ?? undefined}
      />
    </div>
  );
}
