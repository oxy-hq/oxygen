import {
  Building2,
  CalendarDays,
  CircleAlert,
  FolderOpen,
  GitBranch,
  Loader2,
  Pencil,
  Trash2,
  User,
  Users
} from "lucide-react";
import { useEffect, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
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
import {
  useAdminWorkspaceDetail,
  useDeleteAdminWorkspace,
  useRenameAdminWorkspace
} from "@/hooks/api/adminTenants/useAdminWorkspaces";
import ROUTES from "@/libs/utils/routes";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import { AdminAsync } from "../../components/AdminAsync";
import { AdminDetailEyebrow, AdminDetailHeader } from "../../components/AdminDetailHeader";
import { AdminDetailStats } from "../../components/AdminDetailStats";
import { AdminDetailTabPanel, AdminDetailTabs } from "../../components/AdminDetailTabs";
import { AdminEmptyState } from "../../components/AdminEmptyState";
import { AdminLinkedList, AdminLinkedRow } from "../../components/AdminLinkedRow";
import { AdminSectionLabel } from "../../components/AdminSectionLabel";
import { AdminStatusPill, type AdminStatusTone } from "../../components/AdminStatusPill";
import WorkspaceHealthPanel from "./components/WorkspaceHealthPanel";

type TabId = "overview" | "health" | "members" | "settings";

const isTabId = (v: string | null): v is TabId =>
  v === "overview" || v === "health" || v === "members" || v === "settings";

const ageDays = (iso: string) =>
  Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 86_400_000));

const STATUS_MAP: Record<string, { tone: AdminStatusTone; label: string }> = {
  ready: { tone: "ok", label: "Ready" },
  cloning: { tone: "warn", label: "Cloning" },
  failed: { tone: "danger", label: "Failed" },
  not_oxy_project: { tone: "muted", label: "Not Oxy" }
};

/**
 * `/admin/workspaces/:workspaceId` — full-page operator surface for a
 * single workspace. The parent organization is a first-class part of
 * the page identity: it appears in the eyebrow, the subtitle (clickable
 * link), and as its own "Organization" card on the Overview tab. Member
 * rows cross-link to the user detail page, completing the triangle.
 */
export default function AdminWorkspaceDetail({
  workspaceId: workspaceIdProp,
  embedded = false
}: {
  workspaceId?: string;
  embedded?: boolean;
} = {}) {
  const { workspaceId: workspaceIdParam = "" } = useParams<{ workspaceId: string }>();
  const workspaceId = workspaceIdProp ?? workspaceIdParam;
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab");
  const tab: TabId = isTabId(tabParam) ? tabParam : "overview";
  const setTab = (next: TabId) =>
    setSearchParams(
      (prev) => {
        const params = new URLSearchParams(prev);
        if (next === "overview") params.delete("tab");
        else params.set("tab", next);
        return params;
      },
      { replace: true }
    );
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);

  const workspace = useAdminWorkspaceDetail(workspaceId);
  const detail = workspace.data;
  const rename = useRenameAdminWorkspace();
  const remove = useDeleteAdminWorkspace();

  useEffect(() => {
    if (detail) setName(detail.name);
  }, [detail]);

  // Nothing below renders without the workspace, so the gate is the whole page. It used
  // to be `isLoading || !detail` around a spinner, so a *failed* fetch spun forever with
  // no way back but a page reload. `AdminAsync` tells loading from failure and offers a
  // Retry; the guard covers every non-success state, so its callback is unreachable.
  if (!detail) {
    return (
      <div className={embedded ? "p-4" : "mx-auto max-w-7xl p-6 lg:px-10 lg:py-10"}>
        <AdminAsync query={workspace} noun='this workspace' rows={4}>
          {() => null}
        </AdminAsync>
      </div>
    );
  }

  const status = STATUS_MAP[detail.status] ?? { tone: "muted" as const, label: detail.status };
  const lastOpenedDays = detail.last_opened_at ? ageDays(detail.last_opened_at) : null;
  const age = ageDays(detail.created_at);

  const handleSave = () =>
    rename.mutate({ workspaceId: detail.id, name }, { onSuccess: () => setEditing(false) });

  const handleDelete = () => {
    remove.mutate(detail.id, {
      onSuccess: () => {
        setConfirmDelete(false);
        navigate(ROUTES.ADMIN.WORKSPACES);
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
                { label: "Workspaces", to: ROUTES.ADMIN.WORKSPACES },
                { label: detail.name }
              ]}
            />
          )
        }
        icon={FolderOpen}
        title={detail.name}
        subtitle={
          <span className='flex flex-wrap items-center gap-1.5'>
            {detail.org_id && detail.org_name ? (
              <Link
                to={ROUTES.ADMIN.ORG_DETAIL(detail.org_id)}
                className='inline-flex items-center gap-1.5 text-foreground/80 hover:text-foreground'
              >
                <Building2 className='size-3.5' />
                <span>{detail.org_name}</span>
                {detail.org_slug ? (
                  <span className='font-mono text-muted-foreground text-xs'>
                    /{detail.org_slug}
                  </span>
                ) : null}
              </Link>
            ) : (
              <span className='text-muted-foreground'>No parent organization</span>
            )}
            <span aria-hidden>·</span>
            <span className='text-muted-foreground text-xs'>ws</span>
            <CopyableId value={detail.id} />
            {detail.org_id ? (
              <>
                <span aria-hidden>·</span>
                <span className='text-muted-foreground text-xs'>org</span>
                <CopyableId value={detail.org_id} />
              </>
            ) : null}
          </span>
        }
        status={<AdminStatusPill tone={status.tone} label={status.label} />}
        actions={
          <>
            <Button variant='outline' size='sm' onClick={() => setEditing(true)}>
              <Pencil className='size-3.5' />
              Rename
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
            sub: detail.member_count > 0 ? "direct membership" : "via org role only",
            icon: Users
          },
          {
            label: "Last opened",
            value: lastOpenedDays !== null ? `${lastOpenedDays}d` : "—",
            sub: detail.last_opened_at
              ? new Date(detail.last_opened_at).toLocaleDateString()
              : "Never opened",
            icon: CalendarDays
          },
          {
            label: "Age",
            value: age.toLocaleString(),
            sub: `days · since ${new Date(detail.created_at).toLocaleDateString(undefined, {
              year: "numeric",
              month: "short"
            })}`,
            icon: CalendarDays
          }
        ]}
      />

      {detail.error ? (
        <div className='flex items-start gap-3 rounded-lg border border-destructive/40 bg-destructive/5 p-4 text-destructive text-xs'>
          <CircleAlert className='mt-0.5 size-4 shrink-0' />
          <div className='space-y-0.5'>
            <div className='font-semibold'>Workspace error</div>
            <div className='text-destructive/80 text-xs'>{detail.error}</div>
          </div>
        </div>
      ) : null}

      <AdminDetailTabs<TabId>
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "overview", label: "Overview" },
          { id: "health", label: "Health" },
          { id: "members", label: "Members", count: detail.member_count },
          { id: "settings", label: "Settings" }
        ]}
      />

      {tab === "overview" ? (
        <AdminDetailTabPanel>
          <div className='grid gap-6 lg:grid-cols-2'>
            <section className='space-y-3'>
              <AdminSectionLabel>Organization</AdminSectionLabel>
              {detail.org_id && detail.org_name ? (
                <AdminLinkedList>
                  <AdminLinkedRow
                    to={ROUTES.ADMIN.ORG_DETAIL(detail.org_id)}
                    icon={Building2}
                    primary={detail.org_name}
                    secondary={detail.org_slug ? `/${detail.org_slug}` : undefined}
                    meta={<AdminStatusPill tone='info' label='Parent' />}
                  />
                </AdminLinkedList>
              ) : (
                <AdminEmptyState
                  icon={Building2}
                  title='No parent organization'
                  description='This workspace is not attached to any organization.'
                />
              )}

              <AdminSectionLabel className='mt-6'>Source</AdminSectionLabel>
              <div className='space-y-2.5 rounded-lg border border-border/60 bg-card p-4 text-xs'>
                <Field label='Path' icon={FolderOpen} value={detail.path ?? "—"} mono />
                <Field
                  label='Git remote'
                  icon={GitBranch}
                  value={detail.git_remote_url ?? "—"}
                  mono
                />
                <Field
                  label='Last opened'
                  icon={CalendarDays}
                  value={
                    detail.last_opened_at
                      ? new Date(detail.last_opened_at).toLocaleString()
                      : "Never"
                  }
                />
              </div>
            </section>

            <section className='space-y-3'>
              <AdminSectionLabel
                trailing={
                  detail.members.length > 6 ? (
                    <button
                      type='button'
                      onClick={() => setTab("members")}
                      className='font-medium text-[10px] uppercase tracking-[0.14em] hover:text-foreground'
                    >
                      View all →
                    </button>
                  ) : null
                }
              >
                Members
              </AdminSectionLabel>
              {detail.members.length === 0 ? (
                <AdminEmptyState
                  icon={Users}
                  title='No direct members'
                  description='Members of the parent organization may still have access via their org role.'
                />
              ) : (
                <AdminLinkedList>
                  {detail.members.slice(0, 6).map((m) => (
                    <AdminLinkedRow
                      key={m.user_id}
                      to={ROUTES.ADMIN.USER_DETAIL(m.user_id)}
                      icon={User}
                      primary={m.name || m.email}
                      secondary={m.email}
                      meta={<AdminStatusPill tone={roleTone(m.role)} label={m.role} />}
                    />
                  ))}
                </AdminLinkedList>
              )}
            </section>
          </div>
        </AdminDetailTabPanel>
      ) : null}

      {tab === "health" ? <WorkspaceHealthPanel workspaceId={workspaceId} /> : null}

      {tab === "members" ? (
        <AdminDetailTabPanel>
          <AdminSectionLabel
            trailing={
              <span className='tabular-nums'>{detail.members.length.toLocaleString()} total</span>
            }
          >
            Members
          </AdminSectionLabel>
          {detail.members.length === 0 ? (
            <AdminEmptyState icon={Users} title='No direct members' />
          ) : (
            <AdminLinkedList>
              {detail.members.map((m) => (
                <AdminLinkedRow
                  key={m.user_id}
                  to={ROUTES.ADMIN.USER_DETAIL(m.user_id)}
                  icon={User}
                  primary={m.name || m.email}
                  secondary={`${m.email} · joined ${new Date(m.joined_at).toLocaleDateString()}`}
                  meta={<AdminStatusPill tone={roleTone(m.role)} label={m.role} />}
                />
              ))}
            </AdminLinkedList>
          )}
        </AdminDetailTabPanel>
      ) : null}

      {tab === "settings" ? (
        <AdminDetailTabPanel>
          <section className='space-y-4 rounded-lg border border-border/60 bg-card p-6'>
            <div className='space-y-1'>
              <h3 className='font-semibold text-sm'>Identity</h3>
              <p className='text-muted-foreground text-xs'>
                Rename the workspace. The underlying git repository is untouched.
              </p>
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor='ws-name-settings'>Name</Label>
              <Input id='ws-name-settings' value={name} onChange={(e) => setName(e.target.value)} />
            </div>
            <div className='flex justify-end gap-2 border-border/60 border-t pt-4'>
              <Button
                variant='ghost'
                size='sm'
                onClick={() => setName(detail.name)}
                disabled={rename.isPending}
              >
                Reset
              </Button>
              <Button
                size='sm'
                onClick={handleSave}
                disabled={rename.isPending || name === detail.name}
              >
                {rename.isPending ? <Loader2 className='size-3 animate-spin' /> : null}
                Save changes
              </Button>
            </div>
          </section>

          <section className='space-y-4 rounded-lg border border-destructive/40 bg-destructive/5 p-6'>
            <div className='space-y-1'>
              <h3 className='font-semibold text-destructive text-sm'>Danger zone</h3>
              <p className='text-destructive/80 text-xs'>
                Removing this workspace deletes its files, members, and history. The underlying git
                repository is left alone.
              </p>
            </div>
            <Button
              variant='outline'
              size='sm'
              className='border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive'
              onClick={() => setConfirmDelete(true)}
            >
              <Trash2 className='size-3.5' />
              Delete workspace
            </Button>
          </section>
        </AdminDetailTabPanel>
      ) : null}

      <AlertDialog
        open={editing}
        onOpenChange={(o) => !o && !rename.isPending && setEditing(false)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rename workspace</AlertDialogTitle>
            <AlertDialogDescription>Update the workspace's display name.</AlertDialogDescription>
          </AlertDialogHeader>
          <div className='space-y-1.5'>
            <Label htmlFor='ws-name-edit'>Name</Label>
            <Input id='ws-name-edit' value={name} onChange={(e) => setName(e.target.value)} />
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
            <AlertDialogTitle>Delete workspace?</AlertDialogTitle>
            <AlertDialogDescription>
              <strong>{detail.name}</strong> and all of its files, members, and history will be
              removed. The underlying git repository is untouched. This cannot be undone.
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
    </div>
  );
}

const Field = ({
  label,
  value,
  icon: Icon,
  mono = false
}: {
  label: string;
  value: React.ReactNode;
  icon?: React.ComponentType<{ className?: string }>;
  mono?: boolean;
}) => (
  <div className='grid grid-cols-[110px_1fr] items-center gap-3'>
    <span className='inline-flex items-center gap-1.5 text-muted-foreground text-xs'>
      {Icon ? <Icon className='size-3' /> : null}
      {label}
    </span>
    <span className={`truncate ${mono ? "font-mono text-xs" : "text-xs"}`}>{value}</span>
  </div>
);

const roleTone = (role: string): "ok" | "info" | "muted" =>
  role.toLowerCase() === "owner" ? "ok" : role.toLowerCase() === "admin" ? "info" : "muted";
